//! Re-exports from the shared `tee-watcher` crate.
//!
//! All event watching logic has been extracted into `crates/watcher/`.
//! This module re-exports the public API for backward compatibility with
//! the rest of the operator codebase.

#[allow(unused_imports)]
pub use tee_watcher::{
    parse_log, parse_log_tagged, topic_result_challenged, topic_result_expired,
    topic_result_finalized, topic_result_submitted, topic_dispute_resolved,
    EventWatcher, TEEEvent, TaggedEvent,
};
