//! Re-export config types from meta_core.
//!
//! The config module has moved to `meta_core::config`. This re-export
//! maintains backwards compatibility for internal consumers.

pub use meta_core::config::*;
