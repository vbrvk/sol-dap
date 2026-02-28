//! sol-dap library entrypoint.
//!
//! The project primarily runs as a binary DAP server (`src/main.rs`), but we
//! expose the core modules as a library so integration tests (and external
//! tooling) can exercise the compile/debug/session logic without spawning a
//! subprocess.

pub mod config;
pub mod handler;
pub mod launch;
pub mod session;
pub mod source_map;
pub mod variables;
