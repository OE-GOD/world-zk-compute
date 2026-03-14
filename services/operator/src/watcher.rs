//! Re-exports from the shared `tee-watcher` crate.
//!
//! All event watching logic has been extracted into `crates/watcher/`.
//! This module re-exports the public API for backward compatibility with
//! the rest of the operator codebase.

pub use tee_watcher::{EventWatcher, TEEEvent};
