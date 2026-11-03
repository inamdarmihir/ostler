//! Property tests for `redact_detail`: never panics, is idempotent, never balloons, and never leaves an
//! email-like token or a 4+ digit run behind, in any script. Deterministic (seeded xorshift), no extra crates.

use ostler_memory::redact_detail as redact;

mod prng {
    pub struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> u64 {
            // xorshift64*
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }
        pub fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
            &xs[self.below(xs.len())]
        }
    }
}
use prng::Rng;

const ATOMS: &[&str] = &[
    "a",
    "Z",
    "x",
    "ada",
    "example",
    "com",
    "0",
    "1",
    "7",
    "42",
    "1234",
    "98765",
    "@",
    "@",
    ".",
    ".",
    "?",
    "#",
    "/",
    "://",
    ":",
    "-",
    "_",
    " ",
    " ",
    "\t",
    "\n",
    "(",
    ")",
    "\0",
    "\u{1f}",
    // other scripts' digits
    "٣",
    "٤٥٦٧",
    "४",
    "५६७८९",
    "４",
    "１２３４",
    "½",
    "²",
    "Ⅷ",
    // emoji, combining marks, RTL, bidi controls, zero-width, fullwidth at
    "😀",
    "👩‍👩‍👧",
    "\u{301}",
    "e\u{301}",
    "ש",
    "مرحبا",
    "\u{202e}",
    "\u{200b}",
    "\u{feff}",
    "＠",
    "﹫",
    "日本語",
    "İ",
    "ß",
    "[email]",
    "[number]",
    "?[query]",
];

fn random_string(r: &mut Rng, max_atoms: usize) -> String {
    let n = r.below(max_atoms + 1);
    (0..n).map(|_| *r.pick(ATOMS)).collect()
}

fn is_at(c: char) -> bool {
    matches!(c, '@' | '＠' | '﹫')
}

/// An `@` that is not the first char of its whitespace-delimited word and is followed (somewhere) by an
/// alphanumeric: what a human would call an email-like token.
fn has_email_like(s: &str) -> Option<String> {
    // Our own placeholders are not page text.
    let s = s
        .replace("?[query]", "")
        .replace("[email]", "")
        .replace("[number]", "");
    for w in s.split(char::is_whitespace) {
        let chars: Vec<char> = w.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            if is_at(*c) && i > 0 && chars[i + 1..].iter().any(|c| c.is_alphanumeric()) {
                return Some(w.to_string());
            }
        }
    }
    None
}

fn longest_numeric_run(s: &str) -> usize {
    let (mut best, mut cur) = (0, 0);
    for c in s.chars() {
        if c.is_numeric() {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

fn check(input: &str) {
    let once = redact(input);
    let twice = redact(&once);
    assert_eq!(once, twice, "not idempotent for {input:?}");
    assert!(
        once.len() <= input.len() * 4 + 16,
        "output grew from {} to {} bytes for {input:?}",
        input.len(),
        once.len()
    );
    assert!(
        has_email_like(&once).is_none(),
        "email-like token left in {once:?} (from {input:?})"
    );
    assert!(
        longest_numeric_run(&once) < 4,
        "4+ digit run left in {once:?} (from {input:?})"
    );
}

#[test]
fn random_strings_never_panic_stay_idempotent_bounded_and_clean() {
    for seed in [0x9E3779B97F4A7C15, 1, 0xFEEDFACE, 0x1234_5678_9ABC_DEF1] {
        let mut r = Rng(seed);
        for _ in 0..20_000 {
            let s = random_string(&mut r, 24);
            check(&s);
        }
    }
}

#[test]
fn random_unicode_scalars_never_panic() {
    let mut r = Rng(0xDEADBEEFCAFEF00D);
    for _ in 0..5_000 {
        let n = r.below(64);
        let s: String = (0..n)
            .map(|_| loop {
                if let Some(c) = char::from_u32((r.next() % 0x11_0000) as u32) {
                    break c;
                }
            })
            .collect();
        check(&s);
    }
}

#[test]
fn degenerate_inputs() {
    check("");
    check("@");
    check("@@@@@@");
    check("?????");
    check(&"-".repeat(10_000));
    check(&" ".repeat(10_000));
    check(&"\0".repeat(1000));
    check("....////....////");
    check(&"1 ".repeat(5_000));
    check(&"a@b ".repeat(5_000));
    check(&"x.?".repeat(5_000));
    check(&"😀".repeat(5_000));
    check(&"1\u{301}".repeat(100));
}

#[test]
fn very_long_input_is_linear_enough_and_bounded() {
    let big = "ada@example.com 12345 https://x.test/a?b=c ".repeat(50_000);
    let t = std::time::Instant::now();
    let out = redact(&big);
    assert!(t.elapsed().as_secs() < 5, "took {:?}", t.elapsed());
    assert!(out.len() < big.len());
    assert_eq!(redact(&out), out);
}

#[test]
fn leading_at_does_not_shield_a_later_email() {
    // The first '@' is at index 0 of the word, which used to disable the whole check.
    for s in ["@ada@example.com", "@@a@b.co", "@x a@b.co", "(@a@b.co)"] {
        let o = redact(s);
        assert!(has_email_like(&o).is_none(), "{s:?} -> {o:?}");
    }
}

#[test]
fn an_email_followed_by_a_query_is_still_an_email() {
    for s in [
        "ada@example.com?x=1",
        "ada@example.com#frag",
        "mailto:ada@example.com?subject=hi",
        "ada@example.com/path?q",
    ] {
        let o = redact(s);
        assert!(!o.contains("ada@"), "{s:?} -> {o:?}");
        assert!(!o.contains("example.com"), "{s:?} -> {o:?}");
    }
}

#[test]
fn fullwidth_and_small_at_signs_count_as_emails() {
    for s in ["ada＠example.com bad", "ada﹫example.com bad"] {
        let o = redact(s);
        assert!(!o.contains("ada"), "{s:?} -> {o:?}");
    }
}

#[test]
fn invisible_characters_cannot_split_a_number() {
    for s in [
        "1\u{200b}2\u{200b}3\u{200b}4",
        "1\u{301}2\u{301}3\u{301}4\u{301}5",
        "12\u{feff}34",
        "1\u{ad}2\u{ad}3\u{ad}4",
        "١\u{200d}٢\u{200d}٣\u{200d}٤",
    ] {
        let o = redact(s);
        assert!(
            o.chars().all(|c| !c.is_numeric()),
            "digits survived: {s:?} -> {o:?}"
        );
    }
}

#[test]
fn urls_keep_their_path_but_lose_the_query() {
    assert_eq!(
        redact("see https://x.test/f?id=7&e=a@b.co"),
        "see https://x.test/f?[query]"
    );
    assert_eq!(redact("go to /a/b#top now"), "go to /a/b?[query] now");
    // Non-URL words with a '?' are prose, not URLs.
    assert_eq!(redact("really?"), "really?");
}

#[test]
fn ordinary_messages_are_untouched() {
    for s in [
        "Please enter a valid value",
        "Age must be between 18 and 120",
        "Field is required (max 3 chars)",
        "Password too short: 8 chars minimum",
    ] {
        assert_eq!(redact(s), s);
    }
}
