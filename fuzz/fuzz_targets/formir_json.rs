#![no_main]
//! Untrusted JSON (the page-extracted form IR, agent-supplied values) must parse or fail cleanly, and
//! anything that parses must survive the pure logic around it.
use libfuzzer_sys::fuzz_target;
use ostler_forms::*;

fuzz_target!(|data: &[u8]| {
    if let Ok(ir) = serde_json::from_slice::<FormIr>(data) {
        for form in &ir.forms {
            let _ = form.fingerprint("example.com");
            for f in &form.fields {
                let _ = (f.display(), f.is_sensitive(), f.is_empty_value());
            }
            let values = std::collections::BTreeMap::new();
            let _ = resolve_values(&form.fields, &values);
        }
        let _ = serde_json::to_string(&ir);
    }
});
