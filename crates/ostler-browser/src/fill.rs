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
    /// Same form still showing with validation messages.
    ValidationFailed,
    /// A new form/step appeared (wizard) or the page changed to another form.
    Advanced,
    /// The form went away and/or the URL changed: most likely accepted.
    Submitted,
    /// Nothing observable happened.
    Unchanged,
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

#[derive(Debug, Clone, Serialize)]
pub struct FieldDrift {
    pub r#ref: String,
    pub label: String,
    pub was: String,
    pub now: String,
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default()
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
    forms
        .iter()
        .max_by_key(|f| {
            let vis: Vec<Field> = f
                .fields
                .iter()
                .filter(|x| !x.hidden && !x.disabled)
                .cloned()
                .collect();
            resolve_values(&vis, values).matched.len()
        })
        .or(forms.first())
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

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
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

impl Page {
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

        for _ in 0..max_rounds {
            let ir = self.forms().await?;
            host = host_of(&ir.url);
            let Some(form) = find_form(&ir.forms, form_ref.as_deref(), &pending) else {
                if form_ref.is_none() && results.is_empty() {
                    return Err(BrowserError::Other(
                        "no form fields found on this page".into(),
                    ));
                }
                break;
            };
            form_ref = Some(form.r#ref.clone());
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
            let all_fields: Vec<Field> = form
                .fields
                .iter()
                .filter(|f| !f.hidden && !f.disabled)
                .cloned()
                .collect();
            let candidates: Vec<Field> = all_fields
                .iter()
                .filter(|f| !attempted.contains(&f.r#ref))
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
                self.settle(1500, 80).await;
                continue;
            }

            // 2. Memory suggestions for what is still empty.
            if req.auto != AutoMode::Off {
                if let Some(sug) = sug {
                    let todo: Vec<&Field> = candidates
                        .iter()
                        .filter(|f| {
                            f.is_empty_value()
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
                        self.settle(1500, 80).await;
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
            let vis: Vec<Field> = form
                .fields
                .iter()
                .filter(|f| !f.hidden && !f.disabled)
                .cloned()
                .collect();
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
            for f in &vis {
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
            field_errors: errors,
            page_errors,
            submit,
            step,
            unsettled: self.unsettled_count().saturating_sub(unsettled_before),
            elapsed_ms: t0.elapsed().as_millis() as u64,
        })
    }

    async fn fill_one(&self, planned: &Field, value: &Value, source: &str) -> FieldResult {
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
        };
        match self.fill_field(planned, value).await {
            Ok((status, observed, note)) => {
                result.status = status;
                result.observed = observed;
                result.note = note;
            }
            Err((msg, observed)) => {
                result.note = Some(msg);
                result.observed = observed;
            }
        }
        if planned.is_sensitive() {
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
                            self.settle(2500, 120).await;
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
                    let iso = ostler_forms::normalize_date(&text).map_err(|_| {
                        BrowserError::Other(format!("{r} is read-only and the value is not a date"))
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

    /// Fields that no longer hold what they were verified with (a date picker or script cleared them).
    async fn drifted_fields(&self) -> Vec<FieldDrift> {
        let filled = std::mem::take(&mut self.state.lock().unwrap().filled);
        let mut out = vec![];
        for (r, label, was) in filled {
            if let Ok(f) = self.field(&r).await {
                let now = text_of(&f.value);
                if now != was {
                    out.push(FieldDrift { r#ref: r, label, was, now });
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
                self.settle(1500, 80).await;
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

    /// Click a submit/next control and classify what happened.
    pub async fn submit(&self, form: Option<&str>, control: Option<&str>) -> Result<SubmitOutcome> {
        // The caller has already obtained confirmation (if required); only this window may send data.
        self.arm_submit(true);
        let out = self.submit_inner(form, control).await;
        self.arm_submit(false);
        out
    }

    async fn submit_inner(
        &self,
        form: Option<&str>,
        control: Option<&str>,
    ) -> Result<SubmitOutcome> {
        self.guard().await?;
        let before = self.forms().await?;
        let target = pick_form(&before.forms, form, &BTreeMap::new()).cloned();
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
        let drift = self.drifted_fields().await;
        let action = self.click(&ctl).await?;
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
        let (status, next_form) = if !field_errors.is_empty() || !page_errors.is_empty() {
            (SubmitStatus::ValidationFailed, None)
        } else if same_form.is_some() && !url_changed {
            (SubmitStatus::Unchanged, None)
        } else if after
            .as_ref()
            .is_some_and(|ir| ir.forms.iter().any(|f| f.fields.iter().any(|x| !x.hidden)))
        {
            let nf = after
                .as_ref()
                .and_then(|ir| ir.forms.first())
                .map(|f| f.r#ref.clone());
            (SubmitStatus::Advanced, nf)
        } else {
            (SubmitStatus::Submitted, None)
        };
        let excerpt = if matches!(status, SubmitStatus::Submitted | SubmitStatus::Advanced) {
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
