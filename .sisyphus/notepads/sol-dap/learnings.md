# Learnings

## 2026-02-28 Task 1.1: Project Initialization
- `foundry-compilers` is on crates.io (v0.19.14), NOT in the foundry workspace
- `revm-inspectors` is on crates.io (v0.34.2), NOT in the foundry workspace
- Foundry uses edition 2024, rust-version 1.89
- Foundry patches solar crates from paradigmxyz/solar rev 530f129 — we MUST match these patches
- alloy-primitives v1.5.2 with features: getrandom, rand, map-fxhash, map-foldhash
- revm v34.0.0, revm-inspectors v0.34.2
- serde_json needs "arbitrary_precision" feature to match foundry
- Pinned foundry to commit 6628501ef5b68182bc87617c329f7ab2a286cf76
- dap-rs resolves to commit 913a2a52 (v0.4.1-alpha1)
- Full cargo check takes ~56s on first run (lots of dependencies)
- No dependency conflicts between dap-rs and foundry crates!

## 2026-02-28 Task 1.2: Phase 1 DAP skeleton
- In dap-rs, `Request::{success,error}` consume `self`; for handlers taking `&Request`, use `req.clone().success(...)`.
- `ResponseBody` variants are tagged by `command` (e.g., `ResponseBody::Initialize(types::Capabilities)`, `ResponseBody::Threads(responses::ThreadsResponse)`).
- `Server::send_event(Event::Initialized)` exists; it writes to the same stdout stream via an internal `Arc<Mutex<ServerOutput<_>>>`.

## 2026-02-28 Task 1.3: Foundry launch/debug integration
- `forge test --debug --dump <path>` writes a JSON dump that includes `debug_arena` (flattened `DebugNode`s with full `CallTraceStep`s) and `contracts.identified_contracts`.
- The dump format does not include breakpoints; treat them as empty/default when building a debugger context.
- At foundry rev `6628501`, `foundry-debugger` does not publicly re-export its internal `DebuggerContext`, so consumers may need a local context type composed of `Vec<DebugNode>`, `AddressHashMap<String>`, `ContractSources`, and `Breakpoints`.

## 2026-02-28 Task 1.4: Session state + Launch wiring
- In dap-rs (commit `913a2a52`), `events::StoppedEventBody` does **not** implement `Default`; construct it explicitly when sending `Event::Stopped(...)`.
- `types::StoppedEventReason::Entry` exists and matches the DAP spec's `"entry"` reason.
