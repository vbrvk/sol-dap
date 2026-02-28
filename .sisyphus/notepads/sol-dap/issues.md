# Issues

## 2026-02-28
- `foundry-debugger`'s `DebuggerContext` is not publicly re-exported at rev `6628501`; integration code must define its own context struct (or Foundry would need to export it).
- `forge test --debug --dump` JSON does not include breakpoint data; only `debug_arena` + identified contracts are available from the dump.
- When running `cargo test -- --ignored`, Foundry's compiler output (e.g. "Nothing to compile") can obscure the usual libtest per-test/summary lines for the integration test binary, even though the exit code is 0.
