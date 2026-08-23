//! foxshot-ui — the FoxShot render layer.
//!
//! This crate owns pixels and input events; it owns **no** selection
//! logic. Every selection decision (normalisation, clamping, square lock,
//! nudging, handles) comes from [`foxshot_core::SelectionState`] — the
//! renderer just feeds it events and draws whatever it reports.

mod digits;
mod overlay;
mod selector;

pub use selector::{RegionSelector, UiError};
