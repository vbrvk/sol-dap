# Decisions

## 2026-02-28: Integration testing approach
- Introduce a library target (`src/lib.rs`) that re-exports the existing modules so Rust integration tests can call `launch::compile_and_debug` and drive `session::DebugSession` directly.
- Keep the Foundry fixture self-contained (no `forge-std` dependency) by using plain Solidity `assert`.
