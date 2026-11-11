//! Core of Keep Sticky Notes: local store, Google Keep client and synchronisation engine.
//! This library has no user interface, so that its data-safety rules can be tested in isolation.

pub mod auth;
pub mod creds;
pub mod format;
pub mod keep;
pub mod store;
pub mod sync;
