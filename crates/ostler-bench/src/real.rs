//! Real-site evaluation: `form_fill` against live public form-practice pages, judged by reading the DOM
//! back with our own JavaScript, never by what the tool said.
//!
//! Nothing is submitted: these are other people's servers, so each task navigates, inspects and fills, and
//! stops. That limits what is measured to the part that matters most for trust: *does the page hold what
//! the tool says it holds?* Submit handling is covered against fixtures (`ostler-bench suite`).
//!
//! Per field there are three outcomes:
//! - **ok**: the page holds the expected value;
//! - **flagged**: it does not, and the fill report said so (`UNMATCHED`, `FAILED`, a warning naming the key);
//! - **false claim**: it does not, and the report did not say so. The number that must be zero.
//!
//! The corpus is small and the sites are stable practice pages, not the open web. The sites can change or go
//! away; a task whose page cannot be loaded is reported as `unreachable` and excluded from the rates.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::suite::{new_app, problem_lines};

/// One thing to check after the fill: the key that was sent, how to read the page, what it must hold.
struct Check {
    key: &'static str,
    js: &'static str,
    want: Value,
}

struct Site {
    id: &'static str,
    url: &'static str,
    note: &'static str,
    values: Value,
    checks: Vec<Check>,
}

fn c(key: &'static str, js: &'static str, want: Value) -> Check {
    Check { key, js, want }
}

fn sites() -> Vec<Site> {
    vec![
        Site {
            id: "demoqa_practice_form",
            url: "https://demoqa.com/automation-practice-form",
            note: "React page, ads, custom radios/checkboxes, react-select, unlabeled date and subjects fields",
            values: json!({
                "Name": "Ada", "Last Name": "Lovelace", "Email": "ada@example.com",
                "Gender": "Female", "Mobile(10 Digits)": "5551234567",
                "Sports": true, "Music": true,
                "Current Address": "12 Analytical St",
                "Select State": "NCR"
            }),
            checks: vec![
                c("Name", "document.getElementById('firstName').value", json!("Ada")),
                c("Last Name", "document.getElementById('lastName').value", json!("Lovelace")),
                c("Email", "document.getElementById('userEmail').value", json!("ada@example.com")),
                c("Gender", "(document.querySelector('input[name=gender]:checked')||{}).value", json!("Female")),
                c("Mobile(10 Digits)", "document.getElementById('userNumber').value", json!("5551234567")),
                c("Sports", "document.getElementById('hobbies-checkbox-1').checked", json!(true)),
                c("Music", "document.getElementById('hobbies-checkbox-3').checked", json!(true)),
                c("Current Address", "document.getElementById('currentAddress').value", json!("12 Analytical St")),
                c("Select State", "(document.querySelector('#state [class*=singleValue]')||{}).textContent", json!("NCR")),
            ],
        },
        Site {
            id: "formy_form",
            url: "https://formy-project.herokuapp.com/form",
            note: "plain HTML, a select and a date input",
            values: json!({
                "First name": "Ada", "Last name": "Lovelace", "Job title": "Mathematician",
                "Years of experience": "5-9", "Date": "03/15/2026"
            }),
            checks: vec![
                c("First name", "document.getElementById('first-name').value", json!("Ada")),
                c("Last name", "document.getElementById('last-name').value", json!("Lovelace")),
                c("Job title", "document.getElementById('job-title').value", json!("Mathematician")),
                c("Years of experience", "document.getElementById('select-menu').selectedOptions[0].textContent", json!("5-9")),
                c("Date", "document.getElementById('datepicker').value", json!("03/15/2026")),
            ],
        },
        Site {
            id: "practice_automation_fields",
            url: "https://practice-automation.com/form-fields/",
            note: "text, password, checkbox group, radio group, select, textarea",
            values: json!({
                "Name": "Ada Lovelace", "Password": "correct-horse-battery",
                "fav_drink": ["Milk", "Coffee"], "fav_color": "Blue",
                "Do you like automation?": "Yes",
                "Email": "ada@example.com", "Message": "Hello from the evaluation"
            }),
            checks: vec![
                c("Name", "document.getElementById('name-input').value", json!("Ada Lovelace")),
                c("Password", "document.querySelector('input[type=password]').value", json!("correct-horse-battery")),
                c("fav_drink", "[...document.querySelectorAll('input[name=fav_drink]:checked')].map(e=>e.value).sort()", json!(["Coffee", "Milk"])),
                c("fav_color", "(document.querySelector('input[name=fav_color]:checked')||{}).value", json!("Blue")),
                c("Do you like automation?", "document.getElementById('automation').value", json!("yes")),
                c("Email", "document.getElementById('email').value", json!("ada@example.com")),
                c("Message", "document.getElementById('message').value", json!("Hello from the evaluation")),
            ],
        },
        Site {
            id: "selenium_web_form",
            url: "https://www.selenium.dev/selenium/web/web-form.html",
            note: "every native input type, plus a disabled and a read-only field that must not change",
            values: json!({
                "Text input": "Ada", "Password": "pw12345", "Textarea": "Hello",
                "Dropdown (select)": "Two", "Dropdown (datalist)": "New York",
                "Date picker": "03/15/2026", "my-radio": "Default radio",
                "Example range": 7, "Color picker": "#ff0000",
                "Disabled input": "typed", "Readonly input": "typed",
                "Favourite colour of the moon": "grey"
            }),
            checks: vec![
                c("Text input", "document.querySelector('[name=my-text]').value", json!("Ada")),
                c("Password", "document.querySelector('[name=my-password]').value", json!("pw12345")),
                c("Textarea", "document.querySelector('[name=my-textarea]').value", json!("Hello")),
                c("Dropdown (select)", "document.querySelector('[name=my-select]').value", json!("2")),
                c("Dropdown (datalist)", "document.querySelector('[name=my-datalist]').value", json!("New York")),
                c("Date picker", "document.querySelector('[name=my-date]').value", json!("03/15/2026")),
                c("my-radio", "document.getElementById('my-radio-2').checked", json!(true)),
                c("Example range", "document.querySelector('[name=my-range]').value", json!("7")),
                c("Color picker", "document.querySelector('[name=my-colors]').value", json!("#ff0000")),
                // These two must keep their value; the tool has to say it could not change them.
                c("Disabled input", "document.querySelector('[name=my-disabled]').value", json!("")),
                c("Readonly input", "document.querySelector('[name=my-readonly]').value", json!("Readonly input")),
                // No such field: the page cannot hold it, so the tool must say so (`UNMATCHED`) rather than
                // report success. `null` is what the page "holds" for a field that does not exist.
                c("Favourite colour of the moon", "null", json!("grey")),
            ],
        },
        Site {
            id: "testpages_html_form",
            url: "https://testpages.eviltester.com/pages/forms/html-form/",
            note: "checkbox group with a default-checked box, multi-select, radios, select",
            values: json!({
                "Username": "ada", "Password": "pw12345", "TextArea Comment": "Hello",
                "checkboxes[]": ["Checkbox 1", "Checkbox 2"], "radioval": "radio 3",
                "Multiple Select Values": ["Selection Item 1", "Selection Item 2"],
                "Dropdown": "Drop Down Item 5"
            }),
            checks: vec![
                c("Username", "document.querySelector('[name=username]').value", json!("ada")),
                c("Password", "document.querySelector('[name=password]').value", json!("pw12345")),
                c("TextArea Comment", "document.querySelector('[name=comments]').value", json!("Hello")),
                c("checkboxes[]", "[...document.querySelectorAll('input[name=\"checkboxes[]\"]:checked')].map(e=>e.value).sort()", json!(["cb1", "cb2"])),
                c("radioval", "(document.querySelector('input[name=radioval]:checked')||{}).value", json!("rd3")),
                c("Multiple Select Values", "[...document.querySelector('[name=\"multipleselect[]\"]').selectedOptions].map(e=>e.value).sort()", json!(["ms1", "ms2"])),
                c("Dropdown", "document.querySelector('[name=dropdown]').value", json!("dd5")),
            ],
        },
        Site {
            id: "expandtesting_register",
            url: "https://practice.expandtesting.com/register",
            note: "username plus password and a confirm-password field",
            values: json!({"Username": "adalovelace", "Password": "Passw0rd!x", "Confirm Password": "Passw0rd!x"}),
            checks: vec![
                c("Username", "document.getElementById('username').value", json!("adalovelace")),
                c("Password", "document.getElementById('password').value", json!("Passw0rd!x")),
                c("Confirm Password", "document.getElementById('confirmPassword').value", json!("Passw0rd!x")),
            ],
        },
        Site {
            id: "ultimateqa_contact",
            url: "https://ultimateqa.com/filling-out-forms/",
            note: "two forms on one page, one with a CAPTCHA question",
            values: json!({"Name": "Ada Lovelace", "Message": "Hello from the evaluation"}),
            checks: vec![
                c("Name", "document.getElementById('et_pb_contact_name_0').value", json!("Ada Lovelace")),
                c("Message", "document.getElementById('et_pb_contact_message_0').value", json!("Hello from the evaluation")),
            ],
        },
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldVerdict {
    Ok,
    Flagged,
    FalseClaim,
}

pub struct FieldOutcome {
    pub key: &'static str,
    pub verdict: FieldVerdict,
    pub got: Value,
    pub want: Value,
}

pub struct SiteOutcome {
    pub id: &'static str,
    pub note: &'static str,
    /// `Some(reason)` when the page could not be loaded or the tool errored before any fill.
    pub unreachable: Option<String>,
    pub fields: Vec<FieldOutcome>,
    pub calls: usize,
    pub ms: u128,
    /// Time in `navigate`, `form_inspect` and `form_fill`: where the wall clock goes on a real page.
    pub nav_ms: u128,
    pub inspect_ms: u128,
    pub fill_ms: u128,
    pub transcript: String,
}

/// Does the fill report say anything about `key`? Matches a problem line naming it, case-insensitively.
fn flagged(problems: &[String], key: &str) -> bool {
    let k = key.to_lowercase();
    problems.iter().any(|l| l.to_lowercase().contains(&k))
}

async fn run_site(site: &Site) -> SiteOutcome {
    let t0 = Instant::now();
    let dir = tempfile::tempdir().expect("tempdir");
    // A fresh browser per site, so one page's state or crash cannot leak into the next.
    let app = new_app(dir.path());
    let mut transcript = String::new();
    let mut calls = 0usize;
    let mut out = SiteOutcome {
        id: site.id,
        note: site.note,
        unreachable: None,
        fields: vec![],
        calls: 0,
        ms: 0,
        nav_ms: 0,
        inspect_ms: 0,
        fill_ms: 0,
        transcript: String::new(),
    };

    calls += 1;
    let t = Instant::now();
    let nav = app
        .call(
            "navigate",
            &json!({"url": site.url, "observe": "none", "timeout_ms": 30000}),
        )
        .await;
    out.nav_ms = t.elapsed().as_millis();
    if nav.is_error {
        out.unreachable = Some(format!(
            "navigate: {}",
            nav.text_content().lines().next().unwrap_or("")
        ));
    } else {
        calls += 1;
        let t = Instant::now();
        let ins = app.call("form_inspect", &json!({})).await;
        out.inspect_ms = t.elapsed().as_millis();
        transcript.push_str(&ins.text_content());
        calls += 1;
        let t = Instant::now();
        let fill = app.call("form_fill", &json!({"values": site.values})).await;
        out.fill_ms = t.elapsed().as_millis();
        let ft = fill.text_content();
        transcript.push_str(&format!("\n{ft}\n"));
        let mut problems = problem_lines(&ft);
        if fill.is_error {
            // The whole call failed: nothing was claimed, so every field the page lacks is flagged.
            problems.push(format!("error: {ft}"));
        }
        // Settle: frameworks may revert a value a moment after the tool read it back.
        tokio::time::sleep(Duration::from_millis(800)).await;
        for ch in &site.checks {
            let got = app.page_eval(ch.js).await.unwrap_or(Value::Null);
            let verdict = if got == ch.want {
                FieldVerdict::Ok
            } else if fill.is_error || flagged(&problems, ch.key) {
                FieldVerdict::Flagged
            } else {
                FieldVerdict::FalseClaim
            };
            out.fields.push(FieldOutcome {
                key: ch.key,
                verdict,
                got,
                want: ch.want.clone(),
            });
        }
    }
    app.shutdown().await;
    out.calls = calls;
    out.ms = t0.elapsed().as_millis();
    out.transcript = transcript;
    out
}

pub struct Totals {
    pub sites: usize,
    pub unreachable: usize,
    pub fields: usize,
    pub ok: usize,
    pub flagged: usize,
    pub false_claims: usize,
}

pub fn totals(o: &[SiteOutcome]) -> Totals {
    let n = |v: FieldVerdict| {
        o.iter()
            .flat_map(|s| &s.fields)
            .filter(|f| f.verdict == v)
            .count()
    };
    Totals {
        sites: o.len(),
        unreachable: o.iter().filter(|s| s.unreachable.is_some()).count(),
        fields: o.iter().map(|s| s.fields.len()).sum(),
        ok: n(FieldVerdict::Ok),
        flagged: n(FieldVerdict::Flagged),
        false_claims: n(FieldVerdict::FalseClaim),
    }
}

pub async fn run(only: Option<String>) -> anyhow::Result<Value> {
    let mut out = vec![];
    for site in sites() {
        if only.as_deref().is_some_and(|o| !site.id.contains(o)) {
            continue;
        }
        out.push(run_site(&site).await);
    }
    let verbose = std::env::var("OSTLER_REAL_TRANSCRIPT").is_ok();
    println!(
        "{:<28} {:>7} {:>5} {:>6} {:>8} {:>8} {:>8}  detail",
        "site", "ok", "flag", "FALSE", "nav ms", "insp ms", "fill ms"
    );
    for s in &out {
        if let Some(why) = &s.unreachable {
            println!("{:<28} unreachable: {why}", s.id);
            continue;
        }
        let n = |v: FieldVerdict| s.fields.iter().filter(|f| f.verdict == v).count();
        let detail: Vec<String> = s
            .fields
            .iter()
            .filter(|f| f.verdict != FieldVerdict::Ok)
            .map(|f| {
                format!(
                    "{} {:?}: page has {} want {}",
                    if f.verdict == FieldVerdict::FalseClaim {
                        "FALSE"
                    } else {
                        "flagged"
                    },
                    f.key,
                    f.got,
                    f.want
                )
            })
            .collect();
        println!(
            "{:<28} {:>3}/{:<3} {:>5} {:>6} {:>8} {:>8} {:>8}  {}",
            s.id,
            n(FieldVerdict::Ok),
            s.fields.len(),
            n(FieldVerdict::Flagged),
            n(FieldVerdict::FalseClaim),
            s.nav_ms,
            s.inspect_ms,
            s.fill_ms,
            detail.join("; ")
        );
        if verbose {
            println!("---- {} ----\n{}", s.id, s.transcript);
        }
    }
    let t = totals(&out);
    let reachable = t.sites - t.unreachable;
    println!(
        "\n{} sites ({} unreachable), {} fields: {} filled correctly ({:.0}%), {} flagged honestly, {} FALSE CLAIMS",
        t.sites,
        t.unreachable,
        t.fields,
        t.ok,
        100.0 * t.ok as f64 / t.fields.max(1) as f64,
        t.flagged,
        t.false_claims
    );
    println!(
        "of the fields that were not filled, {:.0}% were reported ({} of {})",
        100.0 * t.flagged as f64 / (t.flagged + t.false_claims).max(1) as f64,
        t.flagged,
        t.flagged + t.false_claims
    );
    let rows: Vec<Value> = out
        .iter()
        .map(|s| {
            json!({
                "site": s.id, "note": s.note, "unreachable": s.unreachable,
                "ms": s.ms, "calls": s.calls,
                "nav_ms": s.nav_ms, "inspect_ms": s.inspect_ms, "fill_ms": s.fill_ms,
                "fields": s.fields.iter().map(|f| json!({
                    "key": f.key, "verdict": format!("{:?}", f.verdict),
                    "page_has": f.got, "expected": f.want,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let result = json!({
        "sites": t.sites, "reachable": reachable, "unreachable": t.unreachable,
        "fields": t.fields, "ok": t.ok, "flagged": t.flagged, "false_claims": t.false_claims,
        "rows": rows,
    });
    if t.false_claims > 0 {
        anyhow::bail!("{} false claims", t.false_claims);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_that_names_the_field_counts_as_flagged_and_silence_does_not() {
        let problems = vec!["UNMATCHED key \"Disabled input\": none".to_string()];
        assert!(flagged(&problems, "disabled input"));
        assert!(!flagged(&problems, "Readonly input"));
        assert!(!flagged(&[], "Name"));
    }

    #[test]
    fn every_check_names_a_key_that_is_sent() {
        for s in sites() {
            for ch in &s.checks {
                assert!(
                    s.values.get(ch.key).is_some(),
                    "{}: check {:?} is not in the values sent",
                    s.id,
                    ch.key
                );
            }
        }
    }
}
