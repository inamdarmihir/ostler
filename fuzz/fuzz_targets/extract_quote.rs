#![no_main]
//! Page text and quote derived from arbitrary input: verification, fragments, completeness.
use libfuzzer_sys::fuzz_target;
use ostler_extract::*;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let (page, quote) = s.split_once('\u{1}').unwrap_or((s, s));
    if let QuoteVerdict::Verified { start, end } = verify_quote(page, quote) {
        assert!(start < end && end <= page.chars().count());
        let _ = fragment_from_verified("https://example.com/#x", page, start, end, &FragmentOptions::default());
    }
    let _ = text_fragment_url(page, quote, None, None, &FragmentOptions::default());
    let _ = assess(3, Some(s));
});
