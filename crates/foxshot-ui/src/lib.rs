//! foxshot-ui — the FoxShot render layer.
//!
//! This crate owns pixels and input events; it owns **no** selection or
//! annotation logic. Every selection decision (normalisation, clamping,
//! square lock, nudging, handles) comes from [`foxshot_core::SelectionState`],
//! and every committed annotation edit — undo and redo included — goes
//! through [`foxshot_core::AnnotationDocument`]. The renderers just feed
//! events in and draw whatever the document reports.

mod digits;
mod editor;
pub mod flatten;
mod overlay;
mod selector;

pub use editor::{Editor, EditorOutcome};
pub use selector::{RegionSelector, UiError};
