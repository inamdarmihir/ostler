//! Compact, token-frugal text renderings of forms and reports.

use std::fmt::Write;

use waymarker_browser::{FillReport, FillStatus, SubmitOutcome, SubmitStatus};
use waymarker_forms::{Field, FieldKind, FormIr};

fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn show(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => q(s),
        serde_json::Value::Null => "\"\"".into(),
        other => other.to_string(),
    }
}

fn field_line(f: &Field) -> String {
    let kind = serde_json::to_value(f.kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let mut s = format!("  {} {} {}", f.r#ref, kind, q(&f.display()));
    let mut flags = vec![];
    if f.required {
        flags.push("required".to_string());
    }
    if f.disabled {
        flags.push("disabled".into());
    }
    if f.readonly {
        flags.push("readonly".into());
    }
    if f.is_sensitive() {
        flags.push("sensitive".into());
    }
    if let Some(a) = &f.autocomplete {
        flags.push(format!("autocomplete={a}"));
    }
    if let Some(p) = &f.pattern {
        flags.push(format!("pattern={p}"));
    }
    if f.min.is_some() || f.max.is_some() {
        flags.push(format!(
            "range={}..{}",
            f.min.as_deref().unwrap_or(""),
            f.max.as_deref().unwrap_or("")
        ));
    }
    if let Some(m) = f.maxlength {
        flags.push(format!("maxlength={m}"));
    }
    if let Some(sec) = &f.section {
        flags.push(format!("group={}", q(sec)));
    }
    if !flags.is_empty() {
        let _ = write!(s, " [{}]", flags.join(", "));
    }
    if !f.is_empty_value() && !f.is_sensitive() {
        let _ = write!(s, " value={}", show(&f.value));
    }
    if !f.options.is_empty() && !matches!(f.kind, FieldKind::Text) {
        let shown: Vec<String> = f
            .options
            .iter()
            .take(12)
            .map(|o| {
                if o.disabled {
                    format!("({})", o.label)
                } else {
                    o.label.clone()
                }
            })
            .collect();
        let more = f.options.len().saturating_sub(12);
        let _ = write!(
            s,
            " options: {}{}",
            shown.join(" | "),
            if more > 0 {
                format!(" … +{more}")
            } else {
                String::new()
            }
        );
    }
    if let Some(e) = &f.error {
        let _ = write!(s, " ERROR: {}", q(e));
    }
    s
}

pub fn forms(ir: &FormIr, annotate: impl Fn(&waymarker_forms::Form) -> String) -> String {
    let host = url::Url::parse(&ir.url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default();
    let mut out = format!("Page: {} — {}\n", ir.title, ir.url);
    if ir.forms.is_empty() {
        out.push_str("No form fields found. Use snapshot to see the page.\n");
    }
    for form in &ir.forms {
        let title = form
            .name
            .clone()
            .or_else(|| form.headings.first().cloned())
            .unwrap_or_default();
        let _ = write!(out, "form [{}] {}", form.r#ref, q(&title));
        if let Some(st) = &form.step {
            let _ = write!(out, " step {}/{}", st.index, st.total);
        }
        let _ = writeln!(out, " fingerprint={}", form.fingerprint(&host));
        let note = annotate(form);
        if !note.is_empty() {
            out.push_str(&note);
        }
        for f in &form.fields {
            out.push_str(&field_line(f));
            out.push('\n');
        }
        for s in &form.submit {
            let _ = writeln!(out, "  {} {} {}", s.r#ref, s.role, q(&s.label));
        }
        for e in &form.errors {
            let _ = writeln!(out, "  PAGE ERROR: {}", q(e));
        }
    }
    out
}

pub fn fill(r: &FillReport) -> String {
    let ok = r
        .fields
        .iter()
        .filter(|f| f.status != FillStatus::Failed)
        .count();
    let mut out = format!(
        "Filled {}/{} fields in {}ms (form {}).\n",
        ok,
        r.fields.len(),
        r.elapsed_ms,
        r.form
    );
    for f in &r.fields {
        let tag = match f.status {
            FillStatus::Verified => "OK",
            FillStatus::Reformatted => "OK (reformatted by page)",
            FillStatus::Unverified => "APPLIED (cannot verify)",
            FillStatus::Failed => "FAILED",
        };
        let _ = write!(out, "  {tag} {} {}", f.r#ref, q(&f.label));
        if f.status == FillStatus::Failed {
            let _ = write!(out, ": {}", f.note.as_deref().unwrap_or("unknown error"));
        } else {
            let _ = write!(out, " = {}", show(&f.observed));
            if f.source != "provided" {
                let _ = write!(out, " <- {}", f.source);
            }
            if let Some(n) = &f.note {
                let _ = write!(out, " ({n})");
            }
        }
        if let Some(e) = &f.error {
            let _ = write!(out, " PAGE SAYS: {}", q(e));
        }
        out.push('\n');
    }
    for u in &r.unmatched {
        let _ = write!(out, "  UNMATCHED key {}: {}", q(&u.key), u.reason);
        if !u.candidates.is_empty() {
            let c: Vec<String> = u
                .candidates
                .iter()
                .map(|c| format!("{} {}", c.r#ref, q(&c.label)))
                .collect();
            let _ = write!(out, " — candidates: {}", c.join("; "));
        }
        out.push('\n');
    }
    if !r.missing_required.is_empty() {
        out.push_str("Still empty and required:\n");
        for m in &r.missing_required {
            let _ = write!(
                out,
                "  {} {}{}",
                m.r#ref,
                q(&m.label),
                if m.sensitive {
                    " (sensitive: ask the user)"
                } else {
                    ""
                }
            );
            if !m.options.is_empty() {
                let _ = write!(out, " options: {}", m.options.join(" | "));
            }
            out.push('\n');
        }
    }
    for e in &r.field_errors {
        if !r
            .fields
            .iter()
            .any(|f| f.r#ref == e.r#ref && f.error.is_some())
        {
            let _ = writeln!(
                out,
                "  PAGE SAYS about {} {}: {}",
                e.r#ref,
                q(&e.label),
                q(&e.message)
            );
        }
    }
    for e in &r.page_errors {
        let _ = writeln!(out, "  PAGE ERROR: {}", q(e));
    }
    if let Some(s) = &r.step {
        let _ = writeln!(out, "Step {}/{}.", s.index, s.total);
    }
    if r.ok() {
        if let Some(c) = r
            .submit
            .iter()
            .find(|c| c.role == "submit")
            .or(r.submit.first())
        {
            let _ = writeln!(
                out,
                "Ready: form_submit (control {} {}).",
                c.r#ref,
                q(&c.label)
            );
        }
    }
    out
}

pub fn submit(o: &SubmitOutcome) -> String {
    let head = match o.status {
        SubmitStatus::ValidationFailed => "NOT SUBMITTED: the page reported validation problems.",
        SubmitStatus::Advanced => "Advanced: a new form or step is showing.",
        SubmitStatus::Submitted => "Submitted: the form is gone and the page changed.",
        SubmitStatus::Unchanged => "Nothing changed: the form is still showing without errors. It may not have been submitted; check with snapshot.",
    };
    let mut out = format!("{head}\nNow at: {} — {}\n", o.action.title, o.action.url);
    for e in &o.field_errors {
        let _ = writeln!(out, "  {} {}: {}", e.r#ref, q(&e.label), q(&e.message));
    }
    for e in &o.page_errors {
        let _ = writeln!(out, "  PAGE ERROR: {}", q(e));
    }
    if !o.excerpt.is_empty() {
        let _ = writeln!(out, "Page says:");
        for l in &o.excerpt {
            let _ = writeln!(out, "  {l}");
        }
    }
    for w in &o.action.warnings {
        let _ = writeln!(out, "Warning: {w}");
    }
    for d in &o.action.dialogs {
        let _ = writeln!(out, "Dialog (auto-accepted): {d}");
    }
    if let (SubmitStatus::Advanced, Some(f)) = (o.status, &o.next_form) {
        let _ = writeln!(out, "Next: form_inspect (form {f}).");
    }
    out
}
