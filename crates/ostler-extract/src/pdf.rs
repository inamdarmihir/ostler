//! Born-digital PDF text extraction: pure logic over bytes, no I/O.
//!
//! [`extract_pdf`] reads the text layer of a PDF page by page using `lopdf`
//! (MIT). It enforces a byte cap and a page cap before doing any real work,
//! and it never panics on malformed input: parser panics are caught and
//! reported as errors.
//!
//! A page whose extracted text has fewer than [`SCANNED_MIN_CHARS`]
//! non-whitespace characters is listed in [`PdfDoc::scanned_pages`]. Such a
//! page most likely has no text layer (a scan or an image), so it needs OCR
//! or another route. A page whose text could not be extracted at all goes in
//! [`PdfDoc::failures`] instead and is not counted as scanned.

use std::any::Any;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Input larger than this is rejected before parsing.
pub const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;

/// Documents with more pages than this are rejected.
pub const MAX_PDF_PAGES: u32 = 300;

/// A page with fewer non-whitespace characters than this is marked scanned.
pub const SCANNED_MIN_CHARS: usize = 20;

/// Text of one successfully extracted page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfPage {
    /// 1-based page number as the document numbers it.
    pub number: u32,
    /// Extracted text, trimmed at both ends. May be empty.
    pub text: String,
}

/// Result of extracting a PDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfDoc {
    /// Every page whose text was extracted, in page order.
    pub pages: Vec<PdfPage>,
    /// Total number of pages in the document.
    pub page_count: u32,
    /// Pages with too little text to be born-digital (see [`SCANNED_MIN_CHARS`]).
    pub scanned_pages: Vec<u32>,
    /// Pages whose text extraction failed, with the reason.
    pub failures: Vec<(u32, String)>,
}

/// Why a PDF could not be extracted at all. Per-page problems are reported
/// in [`PdfDoc::failures`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    /// Input is larger than [`MAX_PDF_BYTES`].
    TooLarge { bytes: usize, limit: usize },
    /// Document has more pages than [`MAX_PDF_PAGES`].
    TooManyPages { pages: u32, limit: u32 },
    /// The bytes are not a PDF that could be parsed, or the parser panicked.
    Malformed(String),
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfError::TooLarge { bytes, limit } => {
                write!(f, "PDF is {bytes} bytes, over the {limit} byte limit")
            }
            PdfError::TooManyPages { pages, limit } => {
                write!(f, "PDF has {pages} pages, over the {limit} page limit")
            }
            PdfError::Malformed(why) => write!(f, "cannot read PDF: {why}"),
        }
    }
}

impl std::error::Error for PdfError {}

/// Extract the text layer of a born-digital PDF.
///
/// Returns [`PdfError`] for oversized input, too many pages, or bytes that do
/// not parse. Never panics on malformed input.
pub fn extract_pdf(bytes: &[u8]) -> Result<PdfDoc, PdfError> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(PdfError::TooLarge {
            bytes: bytes.len(),
            limit: MAX_PDF_BYTES,
        });
    }

    let doc = guarded(|| lopdf::Document::load_mem(bytes).map_err(|e| e.to_string()))
        .map_err(PdfError::Malformed)?;

    let page_map = doc.get_pages();
    let page_count = u32::try_from(page_map.len()).unwrap_or(u32::MAX);
    if page_count > MAX_PDF_PAGES {
        return Err(PdfError::TooManyPages {
            pages: page_count,
            limit: MAX_PDF_PAGES,
        });
    }

    let mut pages = Vec::with_capacity(page_map.len());
    let mut scanned_pages = Vec::new();
    let mut failures = Vec::new();

    for &number in page_map.keys() {
        match guarded(|| doc.extract_text(&[number]).map_err(|e| e.to_string())) {
            Ok(raw) => {
                let text = raw.trim().to_string();
                if !has_text_layer(&text) {
                    scanned_pages.push(number);
                }
                pages.push(PdfPage { number, text });
            }
            Err(why) => failures.push((number, why)),
        }
    }

    Ok(PdfDoc {
        pages,
        page_count,
        scanned_pages,
        failures,
    })
}

/// True when the text has enough non-whitespace characters to count as a
/// born-digital text layer.
fn has_text_layer(text: &str) -> bool {
    non_whitespace_chars(text) >= SCANNED_MIN_CHARS
}

fn non_whitespace_chars(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

/// Run a parser step and turn a panic into an error string. This is a
/// backstop: `lopdf` should return errors, but a panic on hostile input must
/// not take down the caller.
fn guarded<T>(step: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(step)) {
        Ok(result) => result,
        Err(payload) => Err(format!("parser panicked: {}", panic_message(&*payload))),
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "unknown panic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal, valid, uncompressed PDF with one Helvetica line of
    /// text per page. Text must not contain parentheses or backslashes.
    fn build_pdf(page_texts: &[&str]) -> Vec<u8> {
        let n = page_texts.len();
        // Object 1 is the catalog, 2 the page tree, 3 the font. Page i is
        // object 4 + 2i and its content stream is object 5 + 2i.
        let mut objects: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                (0..n)
                    .map(|i| format!("{} 0 R", 4 + 2 * i))
                    .collect::<Vec<_>>()
                    .join(" "),
                n
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        for (i, text) in page_texts.iter().enumerate() {
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                5 + 2 * i
            ));
            let stream = format!("BT /F1 24 Tf 72 720 Td ({text}) Tj ET");
            objects.push(format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ));
        }

        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref_at = out.len();
        let size = objects.len() + 1;
        out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for off in &offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n")
                .as_bytes(),
        );
        out
    }

    #[test]
    fn extracts_text_from_page_one() {
        let pdf = build_pdf(&["Hello from page one of the fixture document", "Hi"]);
        let doc = extract_pdf(&pdf).expect("valid fixture must extract");
        assert_eq!(doc.page_count, 2);
        assert_eq!(doc.pages.len(), 2);
        assert_eq!(doc.pages[0].number, 1);
        assert_eq!(
            doc.pages[0].text,
            "Hello from page one of the fixture document"
        );
        assert_eq!(doc.pages[1].number, 2);
        assert_eq!(doc.scanned_pages, vec![2]);
        assert!(doc.failures.is_empty());
    }

    #[test]
    fn garbage_bytes_return_error_without_panic() {
        let err = extract_pdf(b"this is not a pdf at all, just words").unwrap_err();
        assert!(matches!(err, PdfError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn truncated_pdf_does_not_panic() {
        let pdf = build_pdf(&["Truncated fixture page with enough text to matter"]);
        for cut in [0, 9, pdf.len() / 3, pdf.len() / 2, pdf.len() - 1] {
            // Either outcome is fine; the call must simply return.
            let _ = extract_pdf(&pdf[..cut]);
        }
    }

    #[test]
    fn input_over_byte_cap_is_rejected() {
        let big = vec![b'%'; MAX_PDF_BYTES + 1];
        assert_eq!(
            extract_pdf(&big).unwrap_err(),
            PdfError::TooLarge {
                bytes: MAX_PDF_BYTES + 1,
                limit: MAX_PDF_BYTES,
            }
        );
    }

    #[test]
    fn document_over_page_cap_is_rejected() {
        let texts: Vec<String> = (1..=MAX_PDF_PAGES + 1)
            .map(|n| format!("Filler page {n} with some words on it"))
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let pdf = build_pdf(&refs);
        assert_eq!(
            extract_pdf(&pdf).unwrap_err(),
            PdfError::TooManyPages {
                pages: MAX_PDF_PAGES + 1,
                limit: MAX_PDF_PAGES,
            }
        );
    }

    #[test]
    fn scanned_threshold_counts_non_whitespace() {
        assert!(has_text_layer(&"a".repeat(SCANNED_MIN_CHARS)));
        assert!(!has_text_layer(&"a".repeat(SCANNED_MIN_CHARS - 1)));
        assert!(!has_text_layer(&format!(
            "{}{}",
            "a".repeat(SCANNED_MIN_CHARS - 1),
            "  \n\t "
        )));
        assert!(!has_text_layer(&"a ".repeat(SCANNED_MIN_CHARS - 1)));
        assert!(has_text_layer(&"a ".repeat(SCANNED_MIN_CHARS)));
        assert!(!has_text_layer("   \n  "));
    }
}
