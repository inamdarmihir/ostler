//! PDF text extraction for document ingestion.
//!
//! [`extract`] turns PDF bytes into per-page text. It never panics: the parser
//! runs inside [`std::panic::catch_unwind`] and any panic becomes an entry in
//! [`PdfDoc::errors`]. Input is capped at [`MAX_INPUT_BYTES`] and the page
//! count at [`MAX_PAGES`]; both caps are reported in `errors` when they bite.
//!
//! `scanned` is true when the document has no usable text layer: zero pages,
//! or fewer than [`MIN_CHARS_PER_PAGE`] characters per page on average. Such a
//! document is probably image-only and needs OCR, which this crate does not do.

use std::panic::{catch_unwind, AssertUnwindSafe};

use pdf_extract::{output_doc_page, Document, PlainTextOutput};

/// Largest input accepted, in bytes (20 MiB).
pub const MAX_INPUT_BYTES: usize = 20 * 1024 * 1024;
/// Most pages read from one document.
pub const MAX_PAGES: usize = 500;
/// Average characters per page below which a document counts as scanned.
pub const MIN_CHARS_PER_PAGE: f64 = 40.0;

/// Text of one page. `n` is the 1-based page number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfPage {
    pub n: u32,
    pub text: String,
}

/// Result of extracting one PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfDoc {
    pub pages: Vec<PdfPage>,
    /// True when the document has little or no text layer (see module docs).
    pub scanned: bool,
    /// Human-readable problems: parse failures, panics, per-page failures and
    /// caps that truncated the document. Empty when extraction was clean.
    pub errors: Vec<String>,
}

/// Extract per-page text from PDF `bytes`. Never panics.
pub fn extract(bytes: &[u8]) -> PdfDoc {
    let mut errors = Vec::new();

    let pages = if bytes.len() > MAX_INPUT_BYTES {
        errors.push(format!(
            "input truncated: {} bytes exceeds the {} byte cap; not parsed",
            bytes.len(),
            MAX_INPUT_BYTES
        ));
        Vec::new()
    } else {
        let mut parse_errors = Vec::new();
        let outcome = catch_unwind(AssertUnwindSafe(|| parse(bytes, &mut parse_errors)));
        errors.append(&mut parse_errors);
        match outcome {
            Ok(Ok(pages)) => pages,
            Ok(Err(message)) => {
                errors.push(message);
                Vec::new()
            }
            Err(payload) => {
                errors.push(format!("pdf parser panicked: {}", panic_message(&payload)));
                Vec::new()
            }
        }
    };

    let scanned = is_scanned(&pages);
    PdfDoc {
        pages,
        scanned,
        errors,
    }
}

/// Parse the document and extract each page. Per-page failures are pushed to
/// `errors` and the page is kept with empty text so page numbers stay true.
fn parse(bytes: &[u8], errors: &mut Vec<String>) -> Result<Vec<PdfPage>, String> {
    let mut doc = Document::load_mem(bytes).map_err(|e| format!("pdf parse failed: {e}"))?;
    if doc.is_encrypted() {
        doc.decrypt("").map_err(|e| {
            format!("pdf is encrypted and could not be opened without a password: {e}")
        })?;
    }

    let page_numbers: Vec<u32> = doc.get_pages().keys().copied().collect();
    if page_numbers.len() > MAX_PAGES {
        errors.push(format!(
            "document truncated: {} pages, read the first {MAX_PAGES}",
            page_numbers.len()
        ));
    }

    let mut pages = Vec::with_capacity(page_numbers.len().min(MAX_PAGES));
    for n in page_numbers.into_iter().take(MAX_PAGES) {
        let mut text = String::new();
        let result = {
            let mut output = PlainTextOutput::new(&mut text);
            output_doc_page(&doc, &mut output, n)
        };
        if let Err(e) = result {
            errors.push(format!("page {n}: text extraction failed: {e}"));
            text.clear();
        }
        pages.push(PdfPage { n, text });
    }
    Ok(pages)
}

fn is_scanned(pages: &[PdfPage]) -> bool {
    if pages.is_empty() {
        return true;
    }
    let total: usize = pages.iter().map(|p| p.text.chars().count()).sum();
    (total as f64 / pages.len() as f64) < MIN_CHARS_PER_PAGE
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_extract::{Dictionary, Object, Stream};

    fn name(s: &str) -> Object {
        Object::Name(s.as_bytes().to_vec())
    }

    /// Build a PDF with one page per entry. Each page shows its text in
    /// Helvetica; an empty string gives a blank page (like an image-only scan
    /// with no text layer).
    fn build_pdf(page_texts: &[&str]) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(Dictionary::from_iter(vec![
            ("Type", name("Font")),
            ("Subtype", name("Type1")),
            ("BaseFont", name("Helvetica")),
        ]));
        let fonts = Dictionary::from_iter(vec![("F1", Object::Reference(font_id))]);
        let resources = Dictionary::from_iter(vec![("Font", Object::Dictionary(fonts))]);

        let mut kids = Vec::new();
        for text in page_texts {
            let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
            let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
            let page_id = doc.add_object(Dictionary::from_iter(vec![
                ("Type", name("Page")),
                ("Parent", Object::Reference(pages_id)),
                ("Contents", Object::Reference(content_id)),
                ("Resources", Object::Dictionary(resources.clone())),
                (
                    "MediaBox",
                    Object::Array(vec![
                        Object::Integer(0),
                        Object::Integer(0),
                        Object::Integer(612),
                        Object::Integer(792),
                    ]),
                ),
            ]));
            kids.push(Object::Reference(page_id));
        }

        let count = kids.len() as i64;
        doc.objects.insert(
            pages_id,
            Object::Dictionary(Dictionary::from_iter(vec![
                ("Type", name("Pages")),
                ("Kids", Object::Array(kids)),
                ("Count", Object::Integer(count)),
            ])),
        );
        let catalog_id = doc.add_object(Dictionary::from_iter(vec![
            ("Type", name("Catalog")),
            ("Pages", Object::Reference(pages_id)),
        ]));
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let mut out = Vec::new();
        doc.save_to(&mut out).expect("serialise test pdf");
        out
    }

    #[test]
    fn pdf_quote_resolves_to_page_n() {
        let long_a = "Alpha section covers the quarterly revenue figures in detail";
        let long_b = "Beta section lists the regional breakdown and the outlook";
        let bytes = build_pdf(&[long_a, long_b]);

        let doc = extract(&bytes);

        assert!(doc.errors.is_empty(), "unexpected errors: {:?}", doc.errors);
        assert_eq!(doc.pages.len(), 2);
        assert_eq!(doc.pages[0].n, 1);
        assert_eq!(doc.pages[1].n, 2);
        assert!(
            doc.pages[0].text.contains(long_a),
            "page 1 text: {:?}",
            doc.pages[0].text
        );
        assert!(
            doc.pages[1].text.contains(long_b),
            "page 2 text: {:?}",
            doc.pages[1].text
        );
        assert!(!doc.pages[1].text.contains(long_a));
        assert!(!doc.scanned);
    }

    #[test]
    fn scanned_pdf_sets_scanned_flag() {
        let bytes = build_pdf(&["", ""]);

        let doc = extract(&bytes);

        assert_eq!(doc.pages.len(), 2);
        assert!(doc.pages.iter().all(|p| p.text.trim().is_empty()));
        assert!(doc.scanned);
    }

    #[test]
    fn zero_page_pdf_is_scanned() {
        let bytes = build_pdf(&[]);

        let doc = extract(&bytes);

        assert!(doc.pages.is_empty());
        assert!(doc.scanned);
    }

    #[test]
    fn garbage_bytes_yield_error_without_panic() {
        let garbage: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();

        let doc = extract(&garbage);

        assert!(doc.pages.is_empty());
        assert!(doc.scanned);
        assert!(!doc.errors.is_empty());
    }

    #[test]
    fn truncated_pdf_does_not_panic() {
        let bytes = build_pdf(&["Some text that is long enough to be a real page of prose here"]);
        let cut = &bytes[..bytes.len() / 2];

        let doc = extract(cut);

        assert!(doc.pages.len() <= 1);
    }

    #[test]
    fn one_byte_input_does_not_panic() {
        for b in [0u8, b'%', b'P', 0xff] {
            let doc = extract(&[b]);
            assert!(doc.pages.is_empty());
            assert!(!doc.errors.is_empty());
        }
    }

    #[test]
    fn empty_input_does_not_panic() {
        let doc = extract(&[]);
        assert!(doc.pages.is_empty());
        assert!(doc.scanned);
        assert!(!doc.errors.is_empty());
    }

    #[test]
    fn page_count_is_capped_and_reported() {
        let texts: Vec<String> = (0..MAX_PAGES + 1)
            .map(|i| format!("Page number {i} body text"))
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let bytes = build_pdf(&refs);

        let doc = extract(&bytes);

        assert_eq!(doc.pages.len(), MAX_PAGES);
        assert!(
            doc.errors.iter().any(|e| e.contains("truncated")),
            "errors: {:?}",
            doc.errors
        );
    }

    #[test]
    fn oversized_input_is_refused_and_reported() {
        let bytes = vec![b'%'; MAX_INPUT_BYTES + 1];

        let doc = extract(&bytes);

        assert!(doc.pages.is_empty());
        assert!(doc.errors.iter().any(|e| e.contains("truncated")));
    }
}
