//! The form-fill engine: resolve → fill → verify, re-inspecting between fields so
//! dependent controls (country → state) and async option loading just work.

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use async_trait::async_trait;
use ostler_forms::{
    coerce_bool, match_option, normalize_date, normalize_time, resolve_values, Field, FieldKind,
    Form, StepInfo, SubmitControl, Unmatched, ValueError,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::page::{ActionOutcome, BrowserError, Page, Result};

/// Supplies values for fields the agent did not specify (backed by memory).
#[async_trait]
pub trait Suggester: Send + Sync {
    async fn suggest(&self, host: &str, fingerprint: &str, field: &Field) -> Option<Suggestion>;
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub value: Value,
    /// Human-readable provenance, e.g. "memory:email".
    pub source: String,
    pub confidence: f32,
    /// The remembered fact this value came from, and how it was found ("playbook", "autocomplete",
    /// "semantic"). Kept structured so callers never have to parse `source`.
    pub profile_key: Option<String>,
    pub via: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoMode {
    #[default]
    Off,
    Required,
    All,
}

#[derive(Debug, Clone, Default)]
pub struct FillRequest {
    pub form: Option<String>,
    pub values: BTreeMap<String, Value>,
    pub auto: AutoMode,
    pub min_confidence: Option<f32>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FillStatus {
    /// Read back from the page and matches.
    Verified,
    /// Page reformatted the value (masks, casing) but it is equivalent.
    Reformatted,
    /// Applied, but cannot be read back (e.g. file inputs).
    Unverified,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldResult {
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub label: String,
    pub kind: FieldKind,
    pub status: FillStatus,
    pub requested: Value,
    pub observed: Value,
    pub source: String,
    #[serde(skip)]
    pub profile_key: Option<String>,
    #[serde(skip)]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The value took, but the control's own constraints (range, pattern, type) say it is not valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MissingField {
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub label: String,
    pub kind: FieldKind,
    pub sensitive: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldError {
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub label: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FillReport {
    pub form: String,
    pub fingerprint: String,
    pub host: String,
    pub fields: Vec<FieldResult>,
    pub unmatched: Vec<Unmatched>,
    pub missing_required: Vec<MissingField>,
    pub field_errors: Vec<FieldError>,
    pub page_errors: Vec<String>,
    pub submit: Vec<SubmitControl>,
    pub step: Option<StepInfo>,
    /// Other forms on the page that matched the agent's keys exactly as well as `form` did, when the agent
    /// did not choose one. The fill went to the first of them in document order; the agent should say which.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rival_forms: Vec<String>,
    /// Times the page was still busy (network/timers) when we gave up waiting. Non-zero means
    /// verdicts may reflect a page that had not finished updating.
    #[serde(skip_serializing_if = "is_zero")]
    pub unsettled: u64,
    pub elapsed_ms: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl FillReport {
    pub fn failed(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| f.status == FillStatus::Failed)
            .count()
    }
    pub fn ok(&self) -> bool {
        self.failed() == 0
            && self.unmatched.is_empty()
            && self.missing_required.is_empty()
            && self.field_errors.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubmitStatus {
    /// The server (or page) received the submit and answered with problems: the same form is showing with
    /// validation messages, or the page reloaded and says something is invalid/incorrect/failed.
    ValidationFailed,
    /// A new form/step appeared (wizard) or the page changed to another form.
    Advanced,
    /// The form went away and/or the URL changed: most likely accepted.
    Submitted,
    /// Nothing observable happened.
    Unchanged,
    /// The server answered the submit with an HTTP error (4xx/5xx) and showed no validation messages.
    /// The data may or may not have been stored: do not report success and do not blindly retry.
    ServerError,
}

/// Disarms the submit gate when dropped, so a cancelled or timed-out submit (an A2A deadline, a cancelled
/// task) cannot leave the page able to send data.
struct Disarm<'a>(&'a Page);

impl Drop for Disarm<'_> {
    fn drop(&mut self) {
        self.0.arm_submit(false);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmitOutcome {
    pub status: SubmitStatus,
    pub action: ActionOutcome,
    pub field_errors: Vec<FieldError>,
    pub page_errors: Vec<String>,
    /// Headlines/text shown after submission (confirmation messages).
    pub excerpt: Vec<String>,
    pub next_form: Option<String>,
    /// Filled fields the page changed between the fill and the submit click.
    pub drift: Vec<FieldDrift>,
}

/// How long an identical repeat of a submit that may have gone through is refused.
const RESUBMIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
pub struct FieldDrift {
    pub r#ref: String,
    pub label: String,
    pub was: String,
    pub now: String,
}

fn path_of(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| format!("{}{}", u.origin().ascii_serialization(), u.path()))
        .unwrap_or_else(|_| url.to_string())
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default()
}

/// How many of the agent's keys resolve to a field of this form.
fn matched_count(f: &Form, values: &BTreeMap<String, Value>) -> usize {
    resolve_values(&with_options(&f.fields), values)
        .matched
        .len()
}

fn pick_form<'a>(
    forms: &'a [Form],
    want: Option<&str>,
    values: &BTreeMap<String, Value>,
) -> Option<&'a Form> {
    if let Some(r) = want {
        return forms.iter().find(|f| f.r#ref == r.trim_start_matches('@'));
    }
    if values.is_empty() {
        return forms.first();
    }
    // The best match wins; a tie goes to the earlier form in the document, never to the later one.
    let mut best: Option<(&Form, usize)> = None;
    for f in forms {
        let n = matched_count(f, values);
        if best.is_none_or(|(_, b)| n > b) {
            best = Some((f, n));
        }
    }
    best.map(|(f, _)| f)
}

/// The refs of the other forms that match `values` as well as `chosen` does (and at least once).
fn rival_forms(forms: &[Form], chosen: &Form, values: &BTreeMap<String, Value>) -> Vec<String> {
    let n = matched_count(chosen, values);
    if n == 0 {
        return vec![];
    }
    forms
        .iter()
        .filter(|f| f.r#ref != chosen.r#ref && matched_count(f, values) == n)
        .map(|f| f.r#ref.clone())
        .collect()
}

/// The form to work on. If the page re-rendered and our form's ref is gone (React/Vue re-mounts),
/// fall back to the best match for the pending keys instead of silently giving up.
fn find_form<'a>(
    forms: &'a [Form],
    want: Option<&str>,
    values: &BTreeMap<String, Value>,
) -> Option<&'a Form> {
    pick_form(forms, want, values).or_else(|| {
        if want.is_some() && !forms.is_empty() {
            pick_form(forms, None, values)
        } else {
            None
        }
    })
}

/// The visible fields of a form, each radio/checkbox group followed by one pseudo-field per option
/// (ref `<group>#<index>`, kind checkbox) so a single option can be addressed by its own label:
/// `{"College": true}`. Disabled fields are kept (resolution reports them as disabled).
fn with_options(fields: &[Field]) -> Vec<Field> {
    let mut out = Vec::with_capacity(fields.len());
    for f in fields.iter().filter(|f| !f.hidden) {
        out.push(f.clone());
        if f.grouped {
            for (i, o) in f.options.iter().enumerate() {
                out.push(Field {
                    r#ref: format!("{}#{i}", f.r#ref),
                    kind: FieldKind::Checkbox,
                    label: o.label.clone(),
                    name: None,
                    id: None,
                    required: false,
                    disabled: f.disabled || o.disabled,
                    readonly: false,
                    placeholder: None,
                    autocomplete: None,
                    pattern: None,
                    min: None,
                    max: None,
                    step: None,
                    minlength: None,
                    maxlength: None,
                    options: vec![],
                    value: json!(o.selected),
                    section: Some(f.label.clone()),
                    error: None,
                    hidden: false,
                    grouped: false,
                    format: None,
                });
            }
        }
    }
    out
}

/// Parsed date format such as `mm/dd/yyyy`: separator, order of the parts (`d`/`m`/`y`) and the year width.
struct DateFormat {
    sep: char,
    parts: Vec<(char, usize)>,
}

impl DateFormat {
    fn parse(fmt: &str) -> Option<Self> {
        let sep = fmt.chars().find(|c| matches!(c, '/' | '-' | '.'))?;
        let parts: Vec<(char, usize)> = fmt
            .to_lowercase()
            .split(sep)
            .filter_map(|t| Some((t.chars().next()?, t.chars().count())))
            .collect();
        let kinds: HashSet<char> = parts.iter().map(|p| p.0).collect();
        (parts.len() == 3 && kinds.len() == 3 && ['d', 'm', 'y'].iter().all(|k| kinds.contains(k)))
            .then_some(Self { sep, parts })
    }

    /// Already written the way the field wants it (digits, right separator, right widths).
    fn fits(&self, text: &str) -> bool {
        let pieces: Vec<&str> = text.split(self.sep).collect();
        pieces.len() == 3
            && pieces.iter().zip(&self.parts).all(|(p, (k, w))| {
                !p.is_empty()
                    && p.chars().all(|c| c.is_ascii_digit())
                    && if *k == 'y' {
                        p.len() == *w
                    } else {
                        (1..=2).contains(&p.len())
                    }
            })
    }

    fn write(&self, iso: &str) -> String {
        let n: Vec<&str> = iso.split('-').collect();
        let (y, m, d) = (n[0], n[1], n[2]);
        let pieces: Vec<String> = self
            .parts
            .iter()
            .map(|(k, w)| match k {
                'y' if *w == 2 => format!("{:02}", y.parse::<u32>().unwrap_or(0) % 100),
                'y' => y.to_string(),
                'm' => m.to_string(),
                _ => d.to_string(),
            })
            .collect();
        pieces.join(&self.sep.to_string())
    }
}

/// A text box that names a date format wants the date written that way: pages with such widgets rewrite
/// anything else on submit. Returns the value to type, a note when it was rewritten and a warning when the
/// value is not recognisably a date in that shape.
fn plan_date_format(f: &Field, value: &Value) -> (Value, Option<String>, Option<String>) {
    let keep = (value.clone(), None, None);
    let Some(fmt) = f.format.as_deref() else {
        return keep;
    };
    // A read-only box is fronted by a calendar that takes the ISO date; only typeable boxes are rewritten.
    if f.kind != FieldKind::Text || f.readonly || matches!(value, Value::Null | Value::Bool(_)) {
        return keep;
    }
    let assumed = fmt.ends_with('?');
    let fmt = fmt.trim_end_matches('?');
    let Some(df) = DateFormat::parse(fmt) else {
        return keep;
    };
    let text = text_of(value).trim().to_string();
    if text.is_empty() || df.fits(&text) {
        return keep;
    }
    match normalize_date(&text) {
        Ok(iso) => {
            let written = df.write(&iso);
            let note = format!("written as {written} to match the field's date format {fmt}");
            let warn = assumed.then(|| {
                format!("assumed {fmt}, the page did not state a format; check the date landed as intended")
            });
            (json!(written), Some(note), warn)
        }
        Err(_) => (
            value.clone(),
            None,
            Some(format!(
                "value {text:?} does not look like the date format this field expects ({fmt}); the page may rewrite it"
            )),
        ),
    }
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The hidden control a key names, by label (normalised equality, or one containing the other).
fn concealed_match<'a>(key: &str, labels: &'a [String]) -> Option<&'a str> {
    let k = alnum(key);
    if k.len() < 3 {
        return None;
    }
    labels
        .iter()
        .find(|l| {
            let n = alnum(l);
            n == k || (n.len() >= 3 && (n.contains(&k) || k.contains(&n)))
        })
        .map(String::as_str)
}

fn alnum(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn verr(e: ValueError) -> BrowserError {
    BrowserError::Other(e.to_string())
}

/// True while any same-origin iframe on the page has not finished loading its document. An iframe whose
/// `src` is set but still shows the initial `about:blank` counts as loading; cross-origin frames are skipped,
/// since the runtime cannot read their fields anyway.
const IFRAMES_LOADING_JS: &str = "[...document.querySelectorAll('iframe')].some(f => { try { \
    const d = f.contentDocument; if (!d) return false; \
    const src = f.getAttribute('src'); \
    return d.readyState !== 'complete' || (!!src && src !== 'about:blank' && d.URL === 'about:blank'); \
    } catch { return false; } })";

impl Page {
    /// Wait up to 5s for same-origin iframes to finish loading. Returns true if any was loading when called,
    /// so the caller knows a second look at the page may find fields that were not there before.
    async fn wait_for_loading_iframes(&self) -> bool {
        let loading = || async {
            self.eval(IFRAMES_LOADING_JS)
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        };
        if !loading().await {
            return false;
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while Instant::now() < deadline && loading().await {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        self.settle(1500, 150).await;
        true
    }

    /// Fill a form from `{key: value}` input (and memory suggestions), verifying each field.
    pub async fn fill_form(
        &self,
        req: FillRequest,
        sug: Option<&dyn Suggester>,
    ) -> Result<FillReport> {
        self.guard().await?;
        let t0 = Instant::now();
        let unsettled_before = self.unsettled_count();
        let mut pending = req.values.clone();
        let mut attempted: HashSet<String> = HashSet::new();
        let mut results: Vec<FieldResult> = vec![];
        let mut form_ref = req.form.clone();
        let mut suggestion_cache: BTreeMap<String, Option<Suggestion>> = BTreeMap::new();
        let min_conf = req.min_confidence.unwrap_or(0.55);
        let mut host = String::new();
        let mut fingerprint = String::new();
        let max_rounds = req.values.len() + 60;
        // Errors the page was already showing before we touched anything, by ref.
        let mut prior_errors: BTreeMap<String, String> = BTreeMap::new();
        let mut rivals: Vec<String> = vec![];
        let mut first_round = true;
        let mut waited_for_frames = false;

        for _ in 0..max_rounds {
            let ir = self.forms().await?;
            host = host_of(&ir.url);
            let Some(form) = find_form(&ir.forms, form_ref.as_deref(), &pending) else {
                // An embedded form whose iframe is still loading has no fields yet: wait for it, then look again.
                if !waited_for_frames && results.is_empty() {
                    waited_for_frames = true;
                    if self.wait_for_loading_iframes().await {
                        continue;
                    }
                }
                if form_ref.is_none() && results.is_empty() {
                    return Err(BrowserError::Other(
                        "no form fields found on this page".into(),
                    ));
                }
                break;
            };
            // Remembered with the document it belongs to: refs restart at e1 on every page.
            let doc = self.current_doc().await;
            self.state.lock().unwrap().last_form = Some(format!("{doc}\u{1f}{}", form.r#ref));
            form_ref = Some(form.r#ref.clone());
            if std::mem::take(&mut first_round) && req.form.is_none() {
                rivals = rival_forms(&ir.forms, form, &pending);
            }
            if results.is_empty() {
                for f in &form.fields {
                    if let Some(e) = &f.error {
                        prior_errors.insert(f.r#ref.clone(), e.clone());
                    }
                }
            }
            fingerprint = form.fingerprint(&host);
            // Resolve against every visible field, including ones already filled. If a key was
            // ambiguous when its rivals were still empty, it must stay ambiguous once they are filled;
            // otherwise the last remaining rival would silently win by elimination.
            // Disabled controls stay in the list so a key that names one is told so instead of "no match".
            let all_fields: Vec<Field> = with_options(&form.fields);
            let candidates: Vec<Field> = all_fields
                .iter()
                .filter(|f| !attempted.contains(&f.r#ref) && !f.disabled)
                .cloned()
                .collect();

            // 1. Explicit values from the agent, in document order.
            let res = resolve_values(&all_fields, &pending);
            let next = res
                .matched
                .iter()
                .find(|(i, _, _)| !attempted.contains(&all_fields[*i].r#ref))
                .cloned();
            if let Some((idx, key, value)) = next {
                let field = all_fields[idx].clone();
                attempted.insert(field.r#ref.clone());
                pending.remove(&key);
                results.push(self.fill_one(&field, &value, "provided").await);
                self.settle_scoped(3000, 80, form_ref.as_deref(), 700).await;
                continue;
            }

            // 2. Memory suggestions for what is still empty.
            if req.auto != AutoMode::Off {
                if let Some(sug) = sug {
                    let todo: Vec<&Field> = candidates
                        .iter()
                        .filter(|f| {
                            f.is_empty_value()
                                && !f.r#ref.contains('#')
                                && !f.is_sensitive()
                                && !f.readonly
                                && f.kind != FieldKind::File
                                && (req.auto == AutoMode::All || f.required)
                        })
                        .collect();
                    // Look up everything not yet cached concurrently, then apply one and re-inspect.
                    let fresh: Vec<&Field> = todo
                        .iter()
                        .copied()
                        .filter(|f| !suggestion_cache.contains_key(&f.r#ref))
                        .collect();
                    let looked_up = futures_util::future::join_all(
                        fresh.iter().map(|f| sug.suggest(&host, &fingerprint, f)),
                    )
                    .await;
                    for (f, s) in fresh.iter().zip(looked_up) {
                        suggestion_cache.insert(f.r#ref.clone(), s);
                    }
                    let next = todo.iter().find_map(|f| {
                        suggestion_cache
                            .get(&f.r#ref)
                            .and_then(|s| s.as_ref())
                            .filter(|s| s.confidence >= min_conf)
                            .map(|s| ((*f).clone(), s.clone()))
                    });
                    if let Some((field, s)) = next {
                        attempted.insert(field.r#ref.clone());
                        let src = format!("{} ({:.2})", s.source, s.confidence);
                        let mut r = self.fill_one(&field, &s.value, &src).await;
                        r.profile_key = s.profile_key.clone();
                        r.via = Some(s.via.clone());
                        results.push(r);
                        self.settle_scoped(3000, 80, form_ref.as_deref(), 700).await;
                        continue;
                    }
                }
            }
            break;
        }

        // Final read of the page to report what is still wrong.
        let ir = self.forms().await?;
        let form = find_form(&ir.forms, form_ref.as_deref(), &pending);
        let (mut unmatched, mut missing, mut errors, mut page_errors, mut submit, mut step) =
            (vec![], vec![], vec![], vec![], vec![], None);
        if let Some(form) = form {
            let vis: Vec<Field> = with_options(&form.fields);
            if !pending.is_empty() {
                unmatched = resolve_values(&vis, &pending).unmatched;
                // Keys that matched a field we already touched are conflicts, not misses.
                let matched_now = resolve_values(&vis, &pending).matched;
                for (i, key, _) in matched_now {
                    unmatched.push(Unmatched {
                        key,
                        reason: format!(
                            "field {} was not filled (earlier attempt failed)",
                            vis[i].r#ref
                        ),
                        candidates: vec![],
                    });
                }
            }
            for f in vis.iter().filter(|f| !f.disabled) {
                let empty = f.is_empty_value();
                if f.required && empty {
                    missing.push(MissingField {
                        r#ref: f.r#ref.clone(),
                        label: f.display(),
                        kind: f.kind,
                        sensitive: f.is_sensitive(),
                        options: f.options.iter().take(12).map(|o| o.label.clone()).collect(),
                    });
                }
                if let Some(e) = &f.error {
                    // Same message as before our fill: pages that only re-validate on submit keep
                    // showing it, so say so rather than letting it read as a fresh failure.
                    let refilled = results.iter().any(|r| {
                        r.r#ref == f.r#ref
                            && matches!(r.status, FillStatus::Verified | FillStatus::Reformatted)
                    });
                    let message = if refilled && prior_errors.get(&f.r#ref) == Some(e) {
                        format!(
                            "{e} [shown before this fill; the page may only re-check on submit]"
                        )
                    } else {
                        e.clone()
                    };
                    errors.push(FieldError {
                        r#ref: f.r#ref.clone(),
                        label: f.display(),
                        message,
                    });
                }
            }
            // A key that names a control which exists but is not showing is "hidden", not "not found".
            for u in &mut unmatched {
                if u.reason.starts_with("no field on the page")
                    || u.reason.starts_with("no confident match")
                {
                    if let Some(name) = concealed_match(&u.key, &form.concealed) {
                        u.reason = format!(
                            "field {name:?} exists but is hidden right now (a collapsed step, or revealed by another answer); fill or select what reveals it first, then retry"
                        );
                    }
                }
            }
            page_errors = form.errors.clone();
            submit = form.submit.clone();
            step = form.step.clone();
        } else {
            // The form vanished (page navigated or re-rendered without it). Never drop keys silently.
            for key in pending.keys() {
                unmatched.push(Unmatched {
                    key: key.clone(),
                    reason: "the form is no longer on the page (it navigated or re-rendered)"
                        .into(),
                    candidates: vec![],
                });
            }
        }
        // Surface each result's current page error too.
        for r in &mut results {
            if let Some(e) = errors.iter().find(|e| e.r#ref == r.r#ref) {
                r.error = Some(e.message.clone());
            }
        }
        Ok(FillReport {
            form: form_ref.unwrap_or_default(),
            fingerprint,
            host,
            fields: results,
            unmatched,
            missing_required: missing,
            rival_forms: rivals,
            field_errors: errors,
            page_errors,
            submit,
            step,
            unsettled: self.unsettled_count().saturating_sub(unsettled_before),
            elapsed_ms: t0.elapsed().as_millis() as u64,
        })
    }

    /// One option of a radio/checkbox group, addressed by its own label.
    async fn fill_option(&self, planned: &Field, value: &Value, source: &str) -> FieldResult {
        let mut result = FieldResult {
            r#ref: planned.r#ref.clone(),
            label: planned.display(),
            kind: FieldKind::Checkbox,
            status: FillStatus::Failed,
            requested: value.clone(),
            observed: Value::Null,
            source: source.to_string(),
            profile_key: None,
            via: None,
            note: None,
            error: None,
            warning: None,
        };
        let run = async {
            let (group, idx) = planned
                .r#ref
                .split_once('#')
                .and_then(|(g, i)| Some((g, i.parse::<usize>().ok()?)))
                .ok_or_else(|| BrowserError::Other("bad option ref".into()))?;
            let want = coerce_bool(value).ok_or_else(|| {
                BrowserError::Other(format!("cannot read {value} as true/false"))
            })?;
            let live = self.field(group).await?;
            // The index came from an earlier inspection; the group may have been re-rendered since. Find the
            // option by its label (keeping the old index when it still carries that label).
            let same = |o: &ostler_forms::FieldOption| {
                alnum(&o.label).to_lowercase() == alnum(&planned.label).to_lowercase()
            };
            let idx = if live.options.get(idx).is_some_and(same) {
                idx
            } else {
                match live.options.iter().position(same) {
                    Some(i) => i,
                    None => {
                        return Err(BrowserError::Other(format!(
                            "option {:?} is no longer in that group (the page changed since it was inspected); inspect again",
                            planned.label
                        )))
                    }
                }
            };
            let opt = &live.options[idx];
            if opt.disabled {
                return Err(BrowserError::NotActionable("that option is disabled".into()));
            }
            if live.kind == FieldKind::Radio {
                if !want {
                    return Err(BrowserError::Other(
                        "a radio option cannot be unchecked; choose another option of the group instead".into(),
                    ));
                }
                if !opt.selected {
                    self.rt_effect("apply", &[json!(group), json!("radio"), json!(idx)])
                        .await?;
                }
            } else if opt.selected != want {
                let keep: Vec<usize> = live
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(i, o)| if *i == idx { want } else { o.selected })
                    .map(|(i, _)| i)
                    .collect();
                self.rt_effect("apply", &[json!(group), json!("multi_select"), json!(keep)])
                    .await?;
            }
            let after = self.field(group).await?;
            Ok::<bool, BrowserError>(after.options.get(idx).is_some_and(|o| o.selected))
        }
        .await;
        match run {
            Ok(on) => {
                result.observed = json!(on);
                if coerce_bool(value) == Some(on) {
                    result.status = FillStatus::Verified;
                } else {
                    result.note = Some(format!("page shows {on} after filling; expected {value}"));
                }
            }
            Err(e) => result.note = Some(e.to_string()),
        }
        result
    }

    async fn fill_one(&self, planned: &Field, value: &Value, source: &str) -> FieldResult {
        if planned.r#ref.contains('#') {
            return self.fill_option(planned, value, source).await;
        }
        // A text date box is written in its own format; the typed value (and the check) use that form.
        let (typed, format_note, format_warning) = plan_date_format(planned, value);
        let mut result = FieldResult {
            r#ref: planned.r#ref.clone(),
            label: planned.display(),
            kind: planned.kind,
            status: FillStatus::Failed,
            requested: value.clone(),
            observed: Value::Null,
            source: source.to_string(),
            profile_key: None,
            via: None,
            note: None,
            error: None,
            warning: None,
        };
        match self.fill_field(planned, &typed).await {
            Ok((status, observed, note)) => {
                result.status = status;
                result.observed = observed;
                result.note = match (note, format_note) {
                    (Some(a), Some(b)) => Some(format!("{a}; {b}")),
                    (a, b) => a.or(b),
                };
            }
            Err((msg, observed)) => {
                result.note = Some(msg);
                result.observed = observed;
            }
        }
        // The value took; does the control itself consider it valid? ("verified" must not hide an invalid value.)
        if result.status != FillStatus::Failed {
            let mut warnings: Vec<String> = format_warning.into_iter().collect();
            if let Ok(Value::String(w)) = self.rt("validity", &[json!(result.r#ref)]).await {
                warnings.push(w);
            }
            if !warnings.is_empty() {
                result.warning = Some(warnings.join("; "));
            }
        }
        if planned.is_sensitive() {
            if result.warning.is_some() {
                result.warning = Some("details hidden: this is a sensitive field".into());
            }
            // Never echo secrets back into transcripts or logs. Notes can quote the value
            // ("page shows X after filling; expected Y"), so they are hidden as well.
            result.requested = json!("***");
            result.observed = json!("***");
            if result.note.is_some() {
                result.note = Some("details hidden: this is a sensitive field".into());
            }
        }
        result
    }

    /// Returns (status, observed value, note) or (error message, observed value).
    async fn fill_field(
        &self,
        planned: &Field,
        value: &Value,
    ) -> std::result::Result<(FillStatus, Value, Option<String>), (String, Value)> {
        let r = planned.r#ref.as_str();
        let observed_now = |p: &Page| {
            let p = p.clone();
            let r = r.to_string();
            async move { p.field(&r).await.map(|f| f.value).unwrap_or(Value::Null) }
        };
        // Always work from the field's live state: options may have loaded since inspection.
        let mut live = self
            .field(r)
            .await
            .map_err(|e| (e.to_string(), Value::Null))?;

        // Several values for a chip / tag style combobox: pick them one by one. The control does not show
        // what it holds in any readable place, so the result is "applied", never "verified".
        if let (FieldKind::Combobox, Value::Array(items)) = (live.kind, value) {
            let mut picked: Vec<String> = vec![];
            for item in items {
                let want = text_of(item);
                match self.fill_combobox(&live, &want).await {
                    Ok(Some(n)) if n.starts_with("no suggestion") => {
                        return Err((
                            format!(
                                "{want:?} was typed but no suggestion list appeared, so it was not added (picked {} of {})",
                                picked.len(),
                                items.len()
                            ),
                            json!(picked),
                        ))
                    }
                    Ok(_) => picked.push(want),
                    Err(e) => {
                        return Err((
                            format!("{e} (picked {} of {})", picked.len(), items.len()),
                            json!(picked),
                        ))
                    }
                }
            }
            return Ok((
                FillStatus::Unverified,
                json!(picked),
                Some(format!(
                    "picked {} options one by one; the page does not expose the selection, so it was not read back",
                    picked.len()
                )),
            ));
        }

        let outcome: std::result::Result<Option<String>, BrowserError> = async {
            match live.kind {
                FieldKind::Select | FieldKind::MultiSelect | FieldKind::Radio => {
                    let wanted: Vec<Value> = match (&live.kind, value) {
                        (FieldKind::MultiSelect, Value::Array(a)) => a.clone(),
                        (FieldKind::MultiSelect, Value::String(s)) if s.contains(',') => {
                            s.split(',').map(|x| json!(x.trim())).collect()
                        }
                        (_, v) => vec![v.clone()],
                    };
                    let mut idx = Vec::new();
                    for w in &wanted {
                        let mut m = match_option(&live.options, w);
                        if matches!(m, Err(ValueError::NoMatch(_))) {
                            // Options may still be loading (dependent dropdowns): wait once and retry.
                            self.settle_scoped(2500, 120, Some(r), 2500).await;
                            live = self.field(r).await?;
                            m = match_option(&live.options, w);
                        }
                        idx.push(m.map_err(verr)?);
                    }
                    if live.kind == FieldKind::Radio {
                        self.rt_effect("apply", &[json!(r), json!("radio"), json!(idx[0])])
                            .await?;
                    } else if live.kind == FieldKind::Select {
                        self.rt_effect("apply", &[json!(r), json!("select"), json!(idx)])
                            .await?;
                    } else {
                        self.rt_effect("apply", &[json!(r), json!("multi_select"), json!(idx)])
                            .await?;
                    }
                    Ok(None)
                }
                FieldKind::Checkbox | FieldKind::Switch => {
                    let b = coerce_bool(value).ok_or_else(|| {
                        BrowserError::Other(format!("cannot read {value} as true/false"))
                    })?;
                    let kind = if live.kind == FieldKind::Switch {
                        "switch"
                    } else {
                        "checkbox"
                    };
                    self.rt_effect("apply", &[json!(r), json!(kind), json!(b)])
                        .await?;
                    Ok(None)
                }
                FieldKind::Date => {
                    self.set_native(r, &normalize_date(&text_of(value)).map_err(verr)?)
                        .await
                }
                FieldKind::Time => {
                    self.set_native(r, &normalize_time(&text_of(value)).map_err(verr)?)
                        .await
                }
                FieldKind::Month => {
                    let d = normalize_date(&format!("{}-01", text_of(value))).map_err(verr)?;
                    self.set_native(r, &d[..7]).await
                }
                FieldKind::DatetimeLocal => {
                    let v = normalize_datetime_local(&text_of(value)).map_err(verr)?;
                    self.set_native(r, &v).await
                }
                FieldKind::Week | FieldKind::Range | FieldKind::Color => {
                    self.set_native(r, &text_of(value)).await
                }
                FieldKind::File => {
                    let paths: Vec<String> = match value {
                        Value::Array(a) => a.iter().map(text_of).collect(),
                        v => vec![text_of(v)],
                    };
                    self.upload(r, &paths).await?;
                    Ok(None)
                }
                FieldKind::Combobox => self.fill_combobox(&live, &text_of(value)).await,
                _ if live.readonly && !live.disabled => {
                    // Read-only text inputs are usually fronted by a calendar popup.
                    let text = text_of(value);
                    let datey = live.format.is_some()
                        || [
                            Some(live.label.as_str()),
                            live.name.as_deref(),
                            live.id.as_deref(),
                            live.placeholder.as_deref(),
                        ]
                        .into_iter()
                        .flatten()
                        .any(|t| {
                            let t = t.to_lowercase();
                            ["date", "calendar", "birth", "dob"].iter().any(|w| t.contains(w))
                        });
                    let iso = ostler_forms::normalize_date(&text).map_err(|_| {
                        BrowserError::Other(if datey {
                            format!("{r} is read-only and looks like a date picker, but {text:?} is not a date")
                        } else {
                            format!("{r} is read-only: the page does not let this field be typed into")
                        })
                    })?;
                    let res = self.rt_effect("pickDate", &[json!(r), json!(iso)]).await?;
                    if res["ok"].as_bool() != Some(true) {
                        return Err(BrowserError::Other(format!(
                            "{r} is read-only; calendar: {}",
                            res["why"].as_str().unwrap_or("failed")
                        )));
                    }
                    Ok(None)
                }
                _ => {
                    let text = text_of(value);
                    if let Some(max) = live.maxlength {
                        if text.chars().count() as i64 > max {
                            return Err(BrowserError::Other(format!(
                                "value is {} characters but the field allows {max}",
                                text.chars().count()
                            )));
                        }
                    }
                    self.type_into(r, &text).await?;
                    self.rt_effect("finish", &[json!(r)]).await?;
                    Ok(None)
                }
            }
        }
        .await;

        let mut note = match outcome {
            Ok(n) => n,
            Err(e) => return Err((e.to_string(), observed_now(self).await)),
        };

        // Verify by reading the field back.
        let after = self
            .read_back(&live)
            .await
            .map_err(|e| (e.to_string(), Value::Null))?;
        let mut verdict = verify(&live, value, &after.value);
        if verdict == Verdict::Mismatch && live.kind.is_textual() {
            // Second strategy: set the value natively and fire the events (some masked or
            // framework-controlled inputs ignore synthesized typing).
            if self.set_native(r, &text_of(value)).await.is_ok() {
                let again = self
                    .field(r)
                    .await
                    .map_err(|e| (e.to_string(), Value::Null))?;
                verdict = verify(&live, value, &again.value);
                if verdict != Verdict::Mismatch {
                    note = Some(
                        "typed input was not accepted; applied via native value setter".into(),
                    );
                }
                self.remember_fill(&live, &verdict, &again.value);
                return finish_verdict(verdict, again.value, note, value);
            }
        }
        self.remember_fill(&live, &verdict, &after.value);
        finish_verdict(verdict, after.value, note, value)
    }

    /// Keep what a field held when it was verified, so `submit` can notice if the page changes it afterwards.
    fn remember_fill(&self, f: &ostler_forms::Field, v: &Verdict, observed: &Value) {
        if *v == Verdict::Mismatch || !(f.kind.is_textual() || f.kind.is_native_value()) {
            return;
        }
        let val = text_of(observed);
        if val.is_empty() {
            return;
        }
        let mut st = self.state.lock().unwrap();
        st.filled.retain(|(r, _, _)| *r != f.r#ref);
        st.filled.push((f.r#ref.clone(), f.display(), val));
    }

    /// Visible "thank you / has been sent" texts on the page right now (empty when the runtime is gone).
    async fn confirmations(&self) -> Vec<String> {
        match self.rt("confirmations", &[]).await {
            Ok(Value::Array(a)) => a
                .into_iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => vec![],
        }
    }

    /// Error-like messages on the page right now, plus which document is showing: (document id, strong, weak).
    /// `strong` ones announce themselves (role=alert, error/flash/danger class); `weak` ones are short text
    /// that says something went wrong. Empty when the runtime is gone.
    async fn problem_texts(&self) -> (String, Vec<String>, Vec<String>) {
        let doc = self
            .rt("docId", &[])
            .await
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let list = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        match self.rt("problems", &[]).await {
            Ok(v) => (doc, list(&v["strong"]), list(&v["weak"])),
            Err(_) => (doc, vec![], vec![]),
        }
    }

    /// Fields that no longer hold what they were verified with (a date picker or script cleared them).
    async fn drifted_fields(&self) -> Vec<FieldDrift> {
        let filled = std::mem::take(&mut self.state.lock().unwrap().filled);
        let mut out = vec![];
        for (r, label, was) in filled {
            if let Ok(f) = self.field(&r).await {
                let now = text_of(&f.value);
                if now != was {
                    out.push(FieldDrift {
                        r#ref: r,
                        label,
                        was,
                        now,
                    });
                }
            }
        }
        out
    }

    /// Re-read a field after filling it. Frameworks often re-mount the control in response to the very
    /// change we made (choosing a plan rebuilds the form), which makes our ref stale. Follow it by
    /// label to its replacement instead of reporting a failure for a fill that worked.
    async fn read_back(&self, planned: &Field) -> Result<Field> {
        match self.field(&planned.r#ref).await {
            Ok(f) => Ok(f),
            Err(e) if e.to_string().contains("stale") => {
                self.settle_scoped(1500, 80, None, 700).await;
                let ir = self.forms().await?;
                let label = planned.display();
                let mut hits = ir
                    .forms
                    .iter()
                    .flat_map(|f| f.fields.iter())
                    .filter(|f| f.kind == planned.kind && f.display() == label);
                match (hits.next(), hits.next()) {
                    (Some(f), None) => Ok(f.clone()),
                    _ => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn set_native(&self, r: &str, v: &str) -> Result<Option<String>> {
        self.rt_effect("setNative", &[json!(r), json!(v)]).await?;
        Ok(None)
    }

    async fn fill_combobox(&self, f: &Field, text: &str) -> Result<Option<String>> {
        let r = f.r#ref.as_str();
        let editable = self
            .eval(&format!("(()=>{{const e=__wm.el({});return /^(INPUT|TEXTAREA)$/.test(e.tagName)||e.isContentEditable}})()", json!(r)))
            .await?
            .as_bool()
            .unwrap_or(false);
        if editable {
            self.type_into(r, text).await?;
        } else {
            self.click(r).await?;
        }
        let pick = self
            .rt_effect("comboPick", &[json!(r), json!(text), json!(1800)])
            .await?;
        if let Some(label) = pick["picked"].as_str() {
            return Ok(Some(format!("picked option {label:?}")));
        }
        let avail: Vec<String> = pick["available"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if editable && avail.is_empty() {
            // Free-text autocomplete with no popup: the typed text stands.
            self.rt_effect("finish", &[json!(r)]).await?;
            return Ok(Some("no suggestion list appeared; kept typed text".into()));
        }
        Err(BrowserError::Other(format!(
            "no option matches {text:?}; available: {}",
            if avail.is_empty() {
                "(none shown)".into()
            } else {
                avail.join(", ")
            }
        )))
    }

    /// Id of the document showing now (empty when the runtime is gone).
    async fn current_doc(&self) -> String {
        self.rt("docId", &[])
            .await
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default()
    }

    /// Click a submit/next control and classify what happened.
    pub async fn submit(&self, form: Option<&str>, control: Option<&str>) -> Result<SubmitOutcome> {
        self.submit_with(form, control, true).await
    }

    /// Like `submit`, but with `repeat == false` refuses an identical submit of the same form within a short
    /// window after one that was accepted, advanced, errored on the server, or whose outcome is unknown.
    pub async fn submit_with(
        &self,
        form: Option<&str>,
        control: Option<&str>,
        repeat: bool,
    ) -> Result<SubmitOutcome> {
        // The caller has already obtained confirmation (if required); only this window may send data.
        self.arm_submit(true);
        let _gate = Disarm(self);
        let out = self.submit_inner(form, control, repeat).await;
        self.state.lock().unwrap().last_form = None;
        out
    }

    async fn submit_inner(
        &self,
        form: Option<&str>,
        control: Option<&str>,
        repeat: bool,
    ) -> Result<SubmitOutcome> {
        self.guard().await?;
        let before = self.forms().await?;
        let cur_doc = self.current_doc().await;
        // Which form: the one named, else the one we last filled, else the only one. With several forms and
        // no hint, refuse: guessing "the first" once sent a header newsletter box instead of the order form.
        let want: Option<String> = form.map(String::from).or_else(|| {
            let last = self.state.lock().unwrap().last_form.clone();
            let (doc, r) = last.as_deref()?.split_once('\u{1f}')?;
            // A form filled on another document (navigated or went back since) says nothing about this one.
            (!cur_doc.is_empty() && doc == cur_doc && before.forms.iter().any(|f| f.r#ref == r))
                .then(|| r.to_string())
        });
        if want.is_none() && control.is_none() && before.forms.len() > 1 {
            let list: Vec<String> = before.forms.iter().map(|f| f.r#ref.clone()).collect();
            return Err(BrowserError::Other(format!(
                "{} forms on this page and none was filled: pass `form` ({}) or a submit `control` ref",
                before.forms.len(),
                list.join(", ")
            )));
        }
        let target = pick_form(&before.forms, want.as_deref(), &BTreeMap::new()).cloned();
        let before_fp = target
            .as_ref()
            .map(|f| f.fingerprint(&host_of(&before.url)));
        let ctl = match control {
            Some(c) => c.trim_start_matches('@').to_string(),
            None => {
                let f = target
                    .as_ref()
                    .ok_or_else(|| BrowserError::Other("no form on this page".into()))?;
                f.submit
                    .iter()
                    .find(|c| c.role == "submit")
                    .or_else(|| f.submit.first())
                    .map(|c| c.r#ref.clone())
                    .ok_or_else(|| {
                        BrowserError::Other("no submit button found; pass a control ref".into())
                    })?
            }
        };
        let key = before_fp
            .clone()
            .unwrap_or_else(|| format!("control {ctl}"));
        if !repeat {
            let recent = self.state.lock().unwrap().last_submit.clone();
            if let Some((k, at)) = recent {
                if k == key && at.elapsed() < RESUBMIT_WINDOW {
                    return Err(BrowserError::NotActionable(format!(
                        "this form was submitted {}s ago and the outcome may already have gone through; \
                         submitting it again could duplicate it. Check the page first; if you really want to \
                         submit again, call form_submit with again=true",
                        at.elapsed().as_secs()
                    )));
                }
            }
        }
        self.state.lock().unwrap().last_submit = Some((key, std::time::Instant::now()));
        let drift = self.drifted_fields().await;
        let confirm_before = self.confirmations().await;
        let (doc_before, strong_before, weak_before) = self.problem_texts().await;
        let action = match self.click(&ctl).await {
            Ok(a) => a,
            Err(e) => {
                // Only an unknown outcome keeps the guard armed; a refusal to click sent nothing.
                if !matches!(e, BrowserError::Uncertain(_) | BrowserError::Cdp(_)) {
                    self.state.lock().unwrap().last_submit = None;
                }
                return Err(e);
            }
        };
        let after = self.forms().await.ok();
        let host = host_of(&action.url);
        let same_form = after.as_ref().and_then(|ir| {
            ir.forms
                .iter()
                .find(|f| Some(f.fingerprint(&host)) == before_fp)
        });
        let mut field_errors = vec![];
        let mut page_errors = vec![];
        if let Some(f) = same_form {
            for x in f.fields.iter().filter(|x| !x.hidden) {
                if let Some(e) = &x.error {
                    field_errors.push(FieldError {
                        r#ref: x.r#ref.clone(),
                        label: x.display(),
                        message: e.clone(),
                    });
                }
            }
            page_errors = f.errors.clone();
        }
        let url_changed = action.url != before.url;
        // A round trip that lands back on the same form (a bad login redirecting to the login page) is a new
        // document even when the URL is unchanged. Whatever error text that document shows is the server's
        // answer to this submit.
        let (doc_after, strong_after, weak_after) = self.problem_texts().await;
        let reloaded = url_changed || (!doc_before.is_empty() && doc_after != doc_before);
        // Plain text ("invalid", "incorrect") only answers this submit when we are back on the same page
        // (a login flash). On a different page (search results quoting an error) it is just content.
        let same_page = path_of(&action.url) == path_of(&before.url);
        if reloaded && same_form.is_some() {
            let fresh =
                strong_after.iter().chain(weak_after.iter().filter(|t| {
                    same_page && !weak_before.contains(t) && !strong_before.contains(t)
                }));
            for t in fresh {
                if !page_errors.contains(t) {
                    page_errors.push(t.clone());
                }
            }
        }
        // Text that says "sent / thank you" and was not there before the click.
        let confirmed: Vec<String> = self
            .confirmations()
            .await
            .into_iter()
            .filter(|t| !confirm_before.contains(t))
            .collect();
        let (status, next_form) = if !field_errors.is_empty() || !page_errors.is_empty() {
            (SubmitStatus::ValidationFailed, None)
        } else if action.status.is_some_and(|s| s >= 400) {
            (SubmitStatus::ServerError, None)
        } else if same_form.is_some() && !url_changed {
            // An XHR form often stays put and shows a confirmation in place.
            if confirmed.is_empty() {
                (SubmitStatus::Unchanged, None)
            } else {
                (SubmitStatus::Submitted, None)
            }
        } else if after.as_ref().is_some_and(|ir| {
            ir.forms.iter().any(|f| f.fields.iter().any(|x| !x.hidden))
                // A confirmation page with a stray input (newsletter box, search) is not a next step:
                // a next step has something required to fill, a step counter or a "next" button.
                && !(!confirmed.is_empty()
                    && !ir.forms.iter().any(|f| {
                        f.step.is_some()
                            || f.submit.iter().any(|c| c.role == "next")
                            || f.fields.iter().any(|x| !x.hidden && x.required)
                    }))
        }) {
            let nf = after
                .as_ref()
                .and_then(|ir| ir.forms.first())
                .map(|f| f.r#ref.clone());
            (SubmitStatus::Advanced, nf)
        } else {
            (SubmitStatus::Submitted, None)
        };
        let mut excerpt: Vec<String> =
            if matches!(status, SubmitStatus::Submitted | SubmitStatus::Advanced) {
                let snap = self
                    .snapshot(crate::page::SnapshotOpts {
                        max_chars: Some(1500),
                        ..Default::default()
                    })
                    .await?;
                snap.text
                    .lines()
                    .filter(|l| {
                        l.trim_start().starts_with("heading") || l.trim_start().starts_with("text")
                    })
                    .take(6)
                    .map(|l| l.trim().to_string())
                    .collect()
            } else {
                vec![]
            };
        if matches!(status, SubmitStatus::Submitted) {
            for t in confirmed.iter().rev() {
                if !excerpt.iter().any(|l| l.contains(t.as_str())) {
                    excerpt.insert(0, format!("text {t:?}"));
                }
            }
        }
        if matches!(
            status,
            SubmitStatus::ValidationFailed | SubmitStatus::Unchanged
        ) {
            self.state.lock().unwrap().last_submit = None;
        }
        Ok(SubmitOutcome {
            status,
            action,
            field_errors,
            page_errors,
            excerpt,
            next_form,
            drift,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Exact,
    Equivalent,
    Mismatch,
    Unverifiable,
}

fn finish_verdict(
    v: Verdict,
    observed: Value,
    note: Option<String>,
    requested: &Value,
) -> std::result::Result<(FillStatus, Value, Option<String>), (String, Value)> {
    match v {
        Verdict::Exact => Ok((FillStatus::Verified, observed, note)),
        Verdict::Equivalent => Ok((
            FillStatus::Reformatted,
            observed,
            note.or(Some("page reformatted the value".into())),
        )),
        Verdict::Unverifiable => Ok((FillStatus::Unverified, observed, note)),
        Verdict::Mismatch => Err((
            format!("page shows {observed} after filling; expected {requested}"),
            observed,
        )),
    }
}

/// Normalize a datetime-local request to `YYYY-MM-DDTHH:MM`, the form the
/// browser reports back. Shared by the write path and `verify`.
fn normalize_datetime_local(t: &str) -> std::result::Result<String, ValueError> {
    let (d, tm) = t
        .split_once('T')
        .or_else(|| t.split_once(' '))
        .unwrap_or((t, "00:00"));
    Ok(format!("{}T{}", normalize_date(d)?, normalize_time(tm)?))
}

fn verify(field: &Field, requested: &Value, observed: &Value) -> Verdict {
    use FieldKind::*;
    match field.kind {
        File => Verdict::Unverifiable,
        Checkbox | Switch => {
            if coerce_bool(requested) == observed.as_bool() {
                Verdict::Exact
            } else {
                Verdict::Mismatch
            }
        }
        Select | Radio => {
            let Ok(i) = match_option(&field.options, requested) else {
                return Verdict::Mismatch;
            };
            if field.options[i].value == text_of(observed) {
                Verdict::Exact
            } else {
                Verdict::Mismatch
            }
        }
        MultiSelect => {
            let wanted: Vec<Value> = match requested {
                Value::Array(a) => a.clone(),
                Value::String(s) if s.contains(',') => {
                    s.split(',').map(|x| json!(x.trim())).collect()
                }
                v => vec![v.clone()],
            };
            let mut want: Vec<String> = wanted
                .iter()
                .filter_map(|w| {
                    match_option(&field.options, w)
                        .ok()
                        .map(|i| field.options[i].value.clone())
                })
                .collect();
            let mut got: Vec<String> = observed
                .as_array()
                .map(|a| a.iter().map(text_of).collect())
                .unwrap_or_default();
            want.sort();
            got.sort();
            if want == got {
                Verdict::Exact
            } else {
                Verdict::Mismatch
            }
        }
        Combobox => {
            let (a, b) = (alnum(&text_of(requested)), alnum(&text_of(observed)));
            if a == b {
                Verdict::Exact
            } else {
                // No substring rule: "United States" must not verify against
                // "United States Minor Outlying Islands".
                Verdict::Mismatch
            }
        }
        Number => {
            match (
                text_of(requested).trim().parse::<f64>(),
                text_of(observed).trim().parse::<f64>(),
            ) {
                (Ok(a), Ok(b)) if (a - b).abs() < 1e-9 => Verdict::Exact,
                _ => Verdict::Mismatch,
            }
        }
        Date | Time | Month | DatetimeLocal | Week | Range | Color => {
            let want = match field.kind {
                Date => normalize_date(&text_of(requested)).ok(),
                Time => normalize_time(&text_of(requested)).ok(),
                DatetimeLocal => normalize_datetime_local(&text_of(requested)).ok(),
                _ => Some(text_of(requested)),
            };
            if want.as_deref().is_some_and(|w| {
                w == text_of(observed)
                    || (field.kind == Month && text_of(observed).starts_with(&w[..w.len().min(7)]))
            }) {
                Verdict::Exact
            } else {
                Verdict::Mismatch
            }
        }
        _ => {
            let (want, got) = (text_of(requested), text_of(observed));
            if want == got {
                Verdict::Exact
            } else if want.trim() == got.trim()
                || (!alnum(&want).is_empty() && alnum(&want) == alnum(&got))
            {
                Verdict::Equivalent
            } else {
                Verdict::Mismatch
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fld(kind: FieldKind) -> Field {
        serde_json::from_value(json!({"ref":"e1","kind":kind,"label":"x"})).unwrap()
    }

    #[test]
    fn date_format_plan() {
        let mut f = fld(FieldKind::Text);
        f.format = Some("mm/dd/yyyy".into());
        let (v, note, warn) = plan_date_format(&f, &json!("2026-11-02"));
        assert_eq!((v, note.is_some(), warn), (json!("11/02/2026"), true, None));
        // already in shape: untouched
        assert_eq!(
            plan_date_format(&f, &json!("11/02/2026")).0,
            json!("11/02/2026")
        );
        // ambiguous order in another shape: not guessed, warned
        let (v, note, warn) = plan_date_format(&f, &json!("02-11-2026"));
        assert_eq!((v, note, warn.is_some()), (json!("02-11-2026"), None, true));
        f.format = Some("dd-mm-yy".into());
        assert_eq!(
            plan_date_format(&f, &json!("2026-11-02")).0,
            json!("02-11-26")
        );
        // no format: nothing happens
        assert_eq!(
            plan_date_format(&fld(FieldKind::Text), &json!("2026-11-02")).0,
            json!("2026-11-02")
        );
        // read-only (calendar-fronted): the ISO date goes through untouched
        let mut ro = fld(FieldKind::Text);
        ro.format = Some("dd/mm/yyyy".into());
        ro.readonly = true;
        assert_eq!(
            plan_date_format(&ro, &json!("2026-03-04")),
            (json!("2026-03-04"), None, None)
        );
        // a format that is only assumed is disclosed
        let mut a = fld(FieldKind::Text);
        a.format = Some("mm/dd/yyyy?".into());
        let (v, _, warn) = plan_date_format(&a, &json!("2026-03-04"));
        assert_eq!(v, json!("03/04/2026"));
        assert!(warn.unwrap().contains("assumed mm/dd/yyyy"));
        // years: two digits expand by rule, out-of-range and negative years are refused, never a panic
        f.format = Some("dd-mm-yy".into());
        assert_eq!(
            plan_date_format(&f, &json!("March 5, 24")).0,
            json!("05-03-24")
        );
        for bad in [
            "March 5, -5",
            "March 5, 0024x",
            "March 5, 3024",
            "March 5, 1850",
        ] {
            let (v, note, warn) = plan_date_format(&f, &json!(bad));
            assert_eq!((v, note, warn.is_some()), (json!(bad), None, true), "{bad}");
        }
        assert_eq!(
            DateFormat::parse("dd-mm-yy").unwrap().write("0005-03-05"),
            "05-03-05"
        );
    }

    #[tokio::test]
    async fn a_reordered_option_group_is_not_filled_blindly() {
        use crate::{Browser, LaunchOptions, Policy};
        if ostler_cdp::find_chrome(None).is_err() {
            return;
        }
        let srv = ostler_fixtures::FixtureServer::start();
        let b = Browser::launch(LaunchOptions::default(), Policy::default())
            .await
            .expect("launch");
        let page = b.page();
        page.navigate(&srv.url("/reorder.html")).await.unwrap();
        let ir = page.forms().await.unwrap();
        let vis = with_options(&ir.forms[0].fields);
        let large = vis.iter().find(|f| f.label == "Medium").unwrap().clone();
        // The page re-renders the group with the last two options swapped after inspection.
        page.eval("(()=>{const g=document.getElementById('g');const ls=g.querySelectorAll('label');g.appendChild(ls[1])})()")
            .await
            .unwrap();
        // Any later inspection (a settle, another tool call) rebuilds the group in its new order.
        page.forms().await.unwrap();
        let r = page.fill_one(&large, &json!(true), "provided").await;
        let live = page
            .eval("document.querySelector('input[name=size]:checked')?.value || ''")
            .await
            .unwrap();
        // Either the right option got picked or the fill says it did not work; never Verified on the wrong one.
        assert_ne!(live, json!("l"), "wrong option selected: {r:?}");
        assert_eq!(live, json!("m"), "{r:?}");
        assert_eq!(r.status, FillStatus::Verified, "{r:?}");
        b.close().await;
    }

    #[test]
    fn verify_text_variants() {
        let f = fld(FieldKind::Tel);
        assert_eq!(
            verify(&f, &json!("5551234567"), &json!("5551234567")),
            Verdict::Exact
        );
        assert_eq!(
            verify(&f, &json!("5551234567"), &json!("(555) 123-4567")),
            Verdict::Equivalent
        );
        assert_eq!(
            verify(&f, &json!("5551234567"), &json!("")),
            Verdict::Mismatch
        );
    }

    #[test]
    fn verify_datetime_local_is_checked() {
        let f = fld(FieldKind::DatetimeLocal);
        let req = json!("2001-03-05T15:30");
        assert_eq!(verify(&f, &req, &json!("2001-03-05T15:30")), Verdict::Exact);
        assert_eq!(
            verify(&f, &req, &json!("2025-01-01T00:00")),
            Verdict::Mismatch
        );
        assert_eq!(verify(&f, &req, &json!("")), Verdict::Mismatch);
    }

    #[test]
    fn verify_combobox_rejects_substring_matches() {
        let f = fld(FieldKind::Combobox);
        assert_eq!(
            verify(&f, &json!("United States"), &json!("United States")),
            Verdict::Exact
        );
        assert_eq!(
            verify(
                &f,
                &json!("United States"),
                &json!("United States Minor Outlying Islands")
            ),
            Verdict::Mismatch
        );
        assert_eq!(verify(&f, &json!(""), &json!("Canada")), Verdict::Mismatch);
    }

    #[test]
    fn verify_bool_and_number() {
        assert_eq!(
            verify(&fld(FieldKind::Checkbox), &json!("yes"), &json!(true)),
            Verdict::Exact
        );
        assert_eq!(
            verify(&fld(FieldKind::Checkbox), &json!(true), &json!(false)),
            Verdict::Mismatch
        );
        assert_eq!(
            verify(&fld(FieldKind::Number), &json!(3), &json!("3.0")),
            Verdict::Exact
        );
    }

    #[test]
    fn verify_select_uses_option_values() {
        let mut f = fld(FieldKind::Select);
        f.options = serde_json::from_value(
            json!([{"value":"de","label":"Germany"},{"value":"fr","label":"France"}]),
        )
        .unwrap();
        assert_eq!(verify(&f, &json!("Germany"), &json!("de")), Verdict::Exact);
        assert_eq!(
            verify(&f, &json!("Germany"), &json!("fr")),
            Verdict::Mismatch
        );
    }
}
