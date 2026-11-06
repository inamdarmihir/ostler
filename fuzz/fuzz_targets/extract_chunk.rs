#![no_main]
//! Arbitrary text through chunking, BM25 and injection detection: no panics, offsets slice back.
use libfuzzer_sys::fuzz_target;
use ostler_extract::*;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let opts = ChunkOptions { target_chars: 80, overlap_chars: 20 };
    let chars: Vec<char> = s.chars().collect();
    let chunks = chunk_text("https://example.com/", s, &opts);
    let mut idx = Bm25Index::new();
    for c in &chunks {
        let sl: String = chars[c.start..c.end].iter().collect();
        assert_eq!(sl, c.text);
        idx.add_chunk(c, "u");
    }
    let hits = idx.rank(s, 5);
    let _ = idx.mmr(&hits, 0.5, 3);
    let _ = detect_injection(s);
});
