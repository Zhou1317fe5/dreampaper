//! Controlled CPU-only fork of the PP-OCR pipeline pieces the sidecar needs.
//!
//! Origin: `ppocr-rs 0.7.3` (Dario Finardi, Apache-2.0), itself a fork of
//! `meibel-ai/paddle-ocr-rs` (Apache-2.0). Only det/cls/rec, their pre/post
//! processing and the crop utilities were kept; layout, tables, formulas,
//! document unwarping and model downloading were dropped on purpose so the
//! shipped binary has no execution-provider features, no network code and no
//! arbitrary model loading. See `THIRD_PARTY.md` next to `Cargo.toml`.
//!
//! Deliberate behaviour differences from the origin are documented at the
//! call sites; every other numeric path is kept identical so results can be
//! compared against the original crate with golden samples.

pub mod base;
pub mod cls;
pub mod det;
pub mod error;
pub mod rec;
pub mod result;
pub mod scale;
pub mod utils;

pub use error::OcrError;
pub use result::{Angle, Point, TextBox, TextLine};
