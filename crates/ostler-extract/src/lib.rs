//! Pure-logic page reading helpers: no I/O, no async, no browser.
//!
//! * [`chunk`]: split page text or markdown into stable, offset-addressed chunks.
//! * [`bm25`]: in-memory BM25 index, diversification, MMR rerank and sparse vectors.
//! * [`pdf`]: born-digital PDF text extraction, page by page, with size and page caps.
//! * [`quote`]: check that a quote really appears in stored page text.
//! * [`fragment`]: build `#:~:text=` URL text fragments for verified quotes.
//! * [`inject`]: heuristic prompt-injection flags (flags only, not a defence).
//! * [`completeness`]: compare extracted row counts with page-stated totals.
//!
//! All offsets in this crate are **character** (Unicode scalar value) offsets
//! into the original text, not byte offsets. Use [`quote::char_slice`] to
//! slice by them.

pub mod bm25;
pub mod chunk;
pub mod completeness;
pub mod fragment;
pub mod inject;
pub mod markdown;
pub mod pdf;
pub mod quote;

pub use bm25::{diversify, Bm25Index, Bm25Params, Fnv1a, Hit, SparseVector, TermHasher, Tokenizer};
pub use chunk::{chunk_text, estimate_tokens, Chunk, ChunkOptions};
pub use completeness::{assess, assess_hints, parse_hints, Assessment, Completeness, Hints};
pub use fragment::{fragment_from_verified, text_fragment_url, FragmentError, FragmentOptions};
pub use inject::{detect_injection, InjectionFlag, InjectionKind};
pub use markdown::plain_text;
pub use quote::{char_slice, verify_quote, QuoteVerdict, MIN_QUOTE_CHARS};
