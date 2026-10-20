//! LocalSplitscreenAnything core library.
//!
//! Everything the orchestrator needs that is not UI: game profiles, the layout engine,
//! launch providers, isolation backends, window management, controller assignment and the
//! session state machine. The command line (`lsa-cli`) and the Tauri app are thin shells
//! over this crate.

pub mod controllers;
pub mod inject;
pub mod ipc;
pub mod isolation;
pub mod layout;
pub mod patch;
pub mod paths;
pub mod profile;
pub mod providers;
pub mod session;
pub mod window;

pub use lsa_proto as proto;
