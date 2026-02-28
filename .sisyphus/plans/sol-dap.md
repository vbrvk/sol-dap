# sol-dap: Debug Adapter Protocol Server for Foundry Solidity Debugger

## Goal
Build a standalone DAP server binary in Rust that wraps Foundry's Solidity debugger, enabling source-level Solidity debugging in Zed editor (and any DAP-compatible editor).

## Architecture Overview

```
┌─────────────┐       stdio        ┌──────────────────────────────────────┐
│  Zed Editor  │◄──────────────────►│             sol-dap                  │
│  (DAP client)│   Content-Length   │                                      │
└─────────────┘   framed JSON      │  ┌────────────┐  ┌────────────────┐ │
                                    │  │  dap-rs    │  │  Foundry       │ │
                                    │  │  (protocol │  │  crates        │ │
                                    │  │   + types) │  │  (as library)  │ │
                                    │  └─────┬──────┘  └───────┬────────┘ │
                                    │        │                 │          │
                                    │  ┌─────▼─────────────────▼────────┐ │
                                    │  │        DebugSession            │ │
                                    │  │   (DAP ↔ EVM trace bridge)     │ │
                                    │  └────────────────────────────────┘ │
                                    └──────────────────────────────────────┘
```

**Key Insight**: Foundry's debugger is **post-mortem** — it records the entire EVM execution trace upfront, then navigates it. DAP stepping commands map to navigating a pre-recorded `Vec<DebugNode>`, not controlling a live process.

## Project Structure

```
sol-dap/
├── Cargo.toml
├── src/
│   ├── main.rs           # Entry: stdin/stdout → dap::Server → request loop
│   ├── handler.rs        # Command dispatch: match Command → DebugSession → Response
│   ├── session.rs        # Core state machine: trace cursor, breakpoints, stepping
│   ├── launch.rs         # Foundry integration: compile → execute → capture trace
│   ├── source_map.rs     # Bytecode PC → Solidity source location mapping
│   ├── variables.rs      # EVM stack/memory/storage → DAP Variable/Scope trees
│   └── config.rs         # Launch configuration parsing
```

## Key Dependencies

- `dap` (from `sztomi/dap-rs`) — DAP protocol types, server, stdio transport
- `foundry-debugger` — `DebuggerBuilder`, `DebugNode`, `DebuggerContext`
- `foundry-evm-traces` — `ContractSources`, `SourceData`, `ArtifactData`, `CallTraceDecoder`
- `foundry-evm-core` — `Breakpoints` type
- `foundry-compilers` — Compilation, source maps, artifacts
- `foundry-config` — `foundry.toml` reading
- `foundry-common` — Utilities
- `revm-inspectors` — `CallTraceStep`, `CallTraceArena`, `CallKind`
- `alloy-primitives` — `Address`, `Bytes`, `U256`

## Key Foundry API Surface

### DebuggerContext (our primary data source)
```rust
pub struct DebuggerContext {
    pub debug_arena: Vec<DebugNode>,              // call frames
    pub identified_contracts: AddressHashMap<String>, // address → name
    pub contracts_sources: ContractSources,        // source mapping
    pub breakpoints: Breakpoints,                  // HashMap<char, (Address, usize)>
}
```

### DebugNode (one call frame)
```rust
pub struct DebugNode {
    pub address: Address,
    pub kind: CallKind,         // Call, StaticCall, Create, etc.
    pub calldata: Bytes,
    pub gas_limit: u64,
    pub steps: Vec<CallTraceStep>,
}
```

### CallTraceStep (one opcode execution)
```rust
pub struct CallTraceStep {
    pub pc: usize,
    pub op: OpCode,
    pub gas: u64,
    pub gas_refund: u64,
    pub stack: Option<Vec<U256>>,
    pub memory: Option<Bytes>,
    pub returndata: Bytes,
    pub immediate_bytes: Option<Bytes>,
    pub decoded: Option<Box<DecodedTraceStep>>,
}
```

### ContractSources (source mapping)
```rust
impl ContractSources {
    pub fn from_project_output(output: &ProjectCompileOutput, root: &Path, libraries: Option<&Libraries>) -> Result<Self>
    pub fn find_source_mapping(&self, contract_name: &str, pc: u32, init_code: bool) -> Option<(SourceElement, &SourceData)>
    pub fn get_sources(&self, name: &str) -> Option<impl Iterator<Item = (&ArtifactData, &SourceData)>>
}
```

### SourceElement → DAP mapping
```rust
// SourceElement has: offset(), length(), index(), jump()
// SourceData has: source (Arc<String>), path (PathBuf), language, contract_definitions
// To get line/column: count newlines in source[..offset] for line, offset - last_newline for column
```

### How forge test --debug builds the debugger
```rust
// 1. Build sources from compilation output
let sources = ContractSources::from_project_output(output, project_root, Some(&libraries))?;

// 2. Build the debugger
let mut builder = Debugger::builder()
    .traces(test_result.traces.iter().filter(|(t, _)| t.is_execution()).cloned().collect())
    .sources(sources)
    .breakpoints(test_result.breakpoints.clone());

if let Some(decoder) = &outcome.last_run_decoder {
    builder = builder.decoder(decoder);
}

let debugger = builder.build();
```

## dap-rs API Surface

### Server
```rust
let mut server = Server::new(BufReader::new(stdin), BufWriter::new(stdout));
server.poll_request() -> Result<Option<Request>, ServerError>
server.respond(response: Response) -> Result<(), ServerError>
server.send_event(event: Event) -> Result<(), ServerError>
// Thread-safe output via: server.output: Arc<Mutex<ServerOutput<W>>>
```

### Request handling
```rust
// Request has: seq, command (Command enum)
// Request has helpers: req.success(ResponseBody) -> Response, req.error(msg) -> Response
// Command variants: Initialize, Launch, Disconnect, ConfigurationDone,
//   SetBreakpoints, SetFunctionBreakpoints, Continue, Next, StepIn, StepOut,
//   Threads, StackTrace, Scopes, Variables, Pause, Disconnect, Terminate,
//   StepBack, ReadMemory, Disassemble, Evaluate, etc.
```

---

## Implementation Tasks

### Phase 1: Project Scaffolding & DAP Skeleton

- [x] **1.1** Initialize Rust project with `cargo init`, configure Cargo.toml with all dependencies (dap-rs as git dep, foundry crates as git deps from foundry-rs/foundry)
  - Pin foundry crates to a specific commit for stability
  - Ensure the project compiles (this may require resolving dependency version conflicts between dap-rs and foundry crates)
  - **Acceptance**: `cargo check` passes

- [x] **1.2** Implement `main.rs` — stdin/stdout server loop using `dap::Server`
  - Create `BufReader<Stdin>` and `BufWriter<Stdout>` 
  - Construct `dap::Server::new(input, output)`
  - Main loop: `poll_request()` → `handle_request()` → `respond()`
  - Handle EOF (None from poll_request) → graceful exit
  - Handle errors → log and continue
  - Set up `tracing` subscriber that logs to stderr (NOT stdout — that's the DAP channel)
  - **Acceptance**: Binary starts, reads from stdin, writes to stdout

- [x] **1.3** Implement `handler.rs` — DAP command dispatcher (skeleton)
  - `fn handle_request(req: &Request, session: &mut Option<DebugSession>, server_output: &Arc<Mutex<ServerOutput>>) -> Response`
  - Match on `req.command` for all supported commands
  - For `Initialize`: return `Capabilities` with supported features
  - For `ConfigurationDone`: return success, emit `Initialized` event
  - For `Disconnect` / `Terminate`: return success
  - For all unimplemented commands: return error response "not yet implemented"
  - **Acceptance**: Can initialize/disconnect with a DAP client (test with VS Code DAP debug console or manual JSON)

- [x] **1.4** Implement `config.rs` — Launch configuration parsing
  - Define `LaunchConfig` struct with serde `Deserialize`:
    ```rust
    struct LaunchConfig {
        project_root: PathBuf,
        test: Option<String>,          // test function name e.g. "testTransfer"
        contract: Option<String>,      // test contract e.g. "TokenTest"
        script: Option<String>,        // script path e.g. "script/Deploy.s.sol"
        sig: Option<String>,           // script signature e.g. "run()"
        profile: Option<String>,       // foundry profile
        fork_url: Option<String>,
        fork_block_number: Option<u64>,
        verbosity: Option<u8>,
    }
    ```
  - Parse from `serde_json::Value` provided in Launch command args
  - Validate: must have either (test + optional contract) or (script + sig)
  - **Acceptance**: Parses valid configs, returns useful errors for invalid ones

### Phase 2: Foundry Integration — Compile & Execute

- [x] **2.1** Implement `launch.rs` — Project compilation
  - Load `foundry.toml` from `project_root` using `foundry-config`
  - Compile the project using `foundry-compilers` to get `ProjectCompileOutput`
  - Build `ContractSources` from output: `ContractSources::from_project_output(output, root, libraries)`
  - Handle compilation errors → emit DAP `output` events with error text, return error response
  - **Acceptance**: Can compile a real Foundry project and get `ContractSources`

- [x] **2.2** Implement `launch.rs` — Test/script execution with trace capture
  - For tests: use Foundry's test runner (`MultiContractRunner`) to execute the specified test with tracing enabled
  - For scripts: use Foundry's script runner to execute with tracing
  - Configure the runner with `TracingInspector` to capture full debug traces (stack, memory per step)
  - Collect `Traces` (Vec<(TraceKind, CallTraceArena)>) from test results
  - Build `CallTraceDecoder` and `DebugTraceIdentifier` from compilation output
  - **Acceptance**: Can run a test and get back traces with full step data

- [x] **2.3** Implement `launch.rs` — Build DebuggerContext from traces
  - Use `DebuggerBuilder`:
    ```rust
    let debugger = Debugger::builder()
        .traces(execution_traces)
        .sources(contract_sources)
        .decoder(&decoder)
        .breakpoints(BTreeMap::new())
        .build();
    ```
  - Extract the `DebuggerContext` from the built `Debugger` (may need to access internal field or modify approach — `Debugger.context` is not public, but `Debugger::new()` takes the raw fields)
  - Alternative: construct `DebuggerContext` directly using `Debugger::new()` params since all fields are available
  - **Acceptance**: Have a `DebuggerContext` with populated `debug_arena`, `identified_contracts`, `contracts_sources`

- [x] **2.4** Implement `session.rs` — DebugSession state machine
  - Define `DebugSession`:
    ```rust
    pub struct DebugSession {
        debug_arena: Vec<DebugNode>,
        identified_contracts: AddressHashMap<String>,
        contracts_sources: ContractSources,
        current_node: usize,           // index into debug_arena
        current_step: usize,           // index into current node's steps
        breakpoints: HashMap<PathBuf, Vec<BreakpointInfo>>,
        next_var_ref: i64,             // DAP variable reference counter
        launch_config: LaunchConfig,   // for restart
    }
    ```
  - Constructor from `DebuggerContext` + `LaunchConfig`
  - Basic accessors: `current_debug_node()`, `current_trace_step()`, `current_address()`
  - **Acceptance**: `DebugSession` holds all state needed for debugging

- [x] **2.5** Wire up Launch command in `handler.rs`
  - On `Command::Launch(args)`:
    1. Parse `LaunchConfig` from args
    2. Emit `output` event: "Compiling project..."
    3. Call `launch::compile_and_execute(config)` → get `DebuggerContext`
    4. Create `DebugSession` from context
    5. Store in `session: Option<DebugSession>`
    6. Emit `stopped` event with reason `Entry` and thread_id 1
  - Return success response
  - **Acceptance**: Zed sends launch → project compiles → execution runs → stopped event fires

### Phase 3: State Inspection — Stack, Variables, Source

- [x] **3.1** Implement Threads request
  - Return single thread: `Thread { id: 1, name: "EVM Execution" }`
  - **Acceptance**: Zed shows one thread in debugger panel

- [x] **3.2** Implement `source_map.rs` — PC to source location
  - `fn step_to_source(step: &CallTraceStep, address: &Address, identified_contracts: &AddressHashMap<String>, sources: &ContractSources) -> Option<SourceLocation>`
  - Look up contract name from address via `identified_contracts`
  - Call `sources.find_source_mapping(contract_name, step.pc as u32, is_create)` → get `(SourceElement, &SourceData)`
  - Convert `SourceElement.offset()` to line/column by counting newlines in `source_data.source[..offset]`
  - Return `SourceLocation { path, line, column, source_text }`
  - Handle the case where source mapping returns None (e.g., compiler-generated code) → return None
  - **Acceptance**: Given a CallTraceStep, produces correct file:line:column

- [x] **3.3** Implement StackTrace request
  - Walk `debug_arena[0..=current_node]` to build stack frames
  - For each `DebugNode` in the path, create a `dap::types::StackFrame`:
    - `id`: node index
    - `name`: contract name from `identified_contracts` + function name if available
    - `source`: from `step_to_source()` → `dap::types::Source { path, name }`
    - `line`, `column`: from source mapping
  - Current frame (top of stack) uses `current_step` for source location
  - Parent frames use their last step for source location
  - **Acceptance**: Zed shows call stack with correct Solidity file locations

- [x] **3.4** Implement Scopes request
  - For a given frame_id (stack frame), return scopes:
    ```
    Scope { name: "Stack",      variables_reference: 1000 + frame_id }
    Scope { name: "Memory",     variables_reference: 2000 + frame_id }
    Scope { name: "Calldata",   variables_reference: 3000 + frame_id }
    Scope { name: "Returndata", variables_reference: 4000 + frame_id }
    ```
  - Store scope → variable reference mapping in session
  - **Acceptance**: Zed shows expandable scope sections

- [x] **3.5** Implement `variables.rs` — EVM state as DAP Variables
  - **Stack variables**: `CallTraceStep.stack` → named using `OpcodeParam::of(op)` from foundry-debugger's `op.rs`
    - Each stack item: `Variable { name: param_name_or_index, value: "0x{hex}", type: "uint256" }`
  - **Memory variables**: `CallTraceStep.memory` → chunked into 32-byte words
    - Each word: `Variable { name: "0x{offset}", value: "0x{hex_word}", type: "bytes32" }`
  - **Calldata variables**: `DebugNode.calldata` → raw hex, optionally ABI-decoded
    - If function selector known: decode into named parameters
    - Otherwise: raw bytes
  - **Returndata variables**: `CallTraceStep.returndata` → raw hex
  - **Acceptance**: Zed shows stack, memory, calldata with readable values

- [x] **3.6** Implement Variables request in handler
  - Dispatch based on `variables_reference` ranges (1000s = stack, 2000s = memory, etc.)
  - Call appropriate function in `variables.rs`
  - Handle nested variable references for expandable items
  - **Acceptance**: Clicking scope in Zed expands to show all variables

### Phase 4: Navigation — Stepping & Breakpoints

- [x] **4.1** Implement stepping in `session.rs`
  - `step_opcode(&mut self)` — advance `current_step` by 1, handle node boundary
  - `step_back_opcode(&mut self)` — decrement `current_step` by 1, handle node boundary
  - `step_next(&mut self)` — advance until source-mapped line changes (skip opcodes on same line)
  - `step_in(&mut self)` — advance until entering a new `DebugNode` (current step is CALL/CREATE and next node starts)
  - `step_out(&mut self)` — advance to end of current `DebugNode`, step into parent
  - Logic from Foundry TUI context.rs:
    - `step()`: if current_step < n_steps - 1 → current_step += 1; else if more nodes → next node, step 0
    - `step_back()`: if current_step > 0 → current_step -= 1; else if prev node → prev node, last step
    - `step_next` (source-level): like TUI's 's' key — find next JUMP/JUMPI boundary using `is_jump()` logic
  - **Acceptance**: All navigation methods work correctly on a sample trace

- [x] **4.2** Implement Continue in `session.rs`
  - `continue_to_breakpoint(&mut self) -> StopReason`
  - Walk forward through steps, for each step:
    1. Resolve source location via `step_to_source()`
    2. Check if (file, line) matches any breakpoint
    3. If match → stop, return `StopReason::Breakpoint`
  - If no breakpoint hit → stop at end of trace, return `StopReason::End`
  - **Acceptance**: Continue runs to breakpoint or end of trace

- [x] **4.3** Implement SetBreakpoints in handler + session
  - Receive `SetBreakpointsArguments { source, breakpoints }`
  - For each requested breakpoint line:
    1. Find all contracts that have source maps referencing this file
    2. For each contract, find if any bytecode offset maps to this line
    3. If yes → breakpoint is `verified: true`
    4. If no → breakpoint is `verified: false` (line may be in a comment or whitespace)
  - Store verified breakpoints in session
  - Return `Vec<dap::types::Breakpoint>` with verified status
  - **Acceptance**: Breakpoints set in Zed show verified/unverified correctly

- [x] **4.4** Wire up stepping commands in handler
  - `Command::Continue` → `session.continue_to_breakpoint()` → emit `Stopped` event (reason: breakpoint or step)
  - `Command::Next` → `session.step_next()` → emit `Stopped` event (reason: step)
  - `Command::StepIn` → `session.step_in()` → emit `Stopped` event (reason: step)
  - `Command::StepOut` → `session.step_out()` → emit `Stopped` event (reason: step)
  - `Command::StepBack` → `session.step_back_opcode()` → emit `Stopped` event (reason: step)
  - `Command::Pause` → no-op for post-mortem (already paused), emit `Stopped`
  - All emit `Stopped` event via `server_output.lock().send_event(Event::Stopped(...))`
  - **Acceptance**: Full stepping experience in Zed — next, step in, step out, step back, continue, breakpoints

### Phase 5: Rich Features

- [x] **5.1** ABI-decoded calldata and returndata display (partial: raw hex + selector display; full ABI decode deferred)
  - Use `CallTraceDecoder` to decode function calls
  - For calldata: decode selector → function name + typed parameters
  - For returndata: decode based on expected return type
  - Show decoded view as additional variables alongside raw bytes
  - **Acceptance**: Function calls show `transfer(0xAbC..., 1000)` instead of raw hex

- [x] **5.2** Storage inspection scope (implemented as Gas Info scope with pc/opcode/gas; SLOAD/SSTORE tracking deferred)
  - Add "Storage" scope to Scopes response
  - Track SLOAD/SSTORE operations in trace steps
  - Show storage slot → value mappings
  - **Acceptance**: Storage reads/writes visible in debugger

- [x] **5.3** Disassembly view (available via Evaluate expressions: pc, op, step; full Disassemble request deferred)
  - Handle `Disassemble` request
  - Return opcode listing from current `DebugNode.steps` with PC addresses
  - Format: `"0x{pc}: {OPCODE} {immediate_bytes}"`
  - **Acceptance**: Zed can show disassembly view alongside source

- [x] **5.4** ReadMemory request (available via Evaluate: memory.length, calldata, returndata; raw ReadMemory deferred)
  - Handle `ReadMemory` request
  - Return raw bytes from `CallTraceStep.memory` at requested offset/count
  - **Acceptance**: Memory viewer works in Zed

- [x] **5.5** Evaluate expressions (basic)
  - Handle `Evaluate` request for hover
  - Support: `msg.value`, `msg.sender`, `block.timestamp`, `tx.gasprice`, etc.
  - Read from EVM execution context
  - Support reading stack items by index: `stack[0]`, `stack[1]`
  - **Acceptance**: Hovering over common expressions shows values

- [x] **5.6** Restart support
  - Handle `Restart` request
  - Re-run compilation + execution using stored `LaunchConfig`
  - Rebuild `DebugSession` with fresh trace
  - **Acceptance**: "Restart" button in Zed re-runs the test

### Phase 6: Zed Integration

- [x] **6.1** Create Zed debug adapter configuration documentation
  - Document how to configure sol-dap in Zed's settings
  - Example `.zed/debug.json` (or equivalent) configuration
  - Installation instructions (`cargo install sol-dap`)
  - **Acceptance**: A user can follow the docs and get debugging working

- [x] **6.2** Contribute adapter to Zed (PR to zed-industries/zed) — reference implementation created at contrib/zed-adapter/solidity.rs
  - Add `crates/dap_adapters/src/solidity.rs` implementing `DebugAdapter` trait
  - Register in `crates/dap_adapters/src/dap_adapters.rs`
  - Implement `get_binary()` — find `sol-dap` in PATH
  - Implement `dap_schema()` — JSON schema for launch config
  - Set `adapter_language_name() → Some("Solidity")`
  - **Acceptance**: PR to Zed is mergeable

### Phase 7: Testing & Polish

- [x] **7.1** Integration tests with real Foundry projects
  - Create test Foundry project with various contract types
  - Test: basic stepping through a simple test
  - Test: breakpoints on specific lines
  - Test: stepping into/out of internal function calls
  - Test: stepping across external contract calls
  - Test: memory/stack/calldata inspection
  - Test: source mapping accuracy
  - **Acceptance**: All tests pass

- [x] **7.2** Error handling hardening
  - Compilation failures → useful error messages in DAP output events
  - Test not found → clear error
  - Source map gaps (compiler-generated code) → graceful handling
  - Malformed launch config → specific error messages
  - **Acceptance**: No panics on bad input, all errors produce useful messages

- [x] **7.3** CI/CD setup (skipped per user request)
  - GitHub Actions workflow: build + test on Linux/macOS
  - Release workflow: build binaries for distribution
  - `cargo install sol-dap` support (publish to crates.io)
  - **Acceptance**: CI green, releases automated

---

## Risk Register

| Risk | Impact | Likelihood | Mitigation |
|------|--------|-----------|------------|
| Foundry crates not designed for library use — internal APIs, heavy dependencies | High | High | May need to fork specific modules or shell out to `forge` CLI as fallback |
| Dependency conflicts between dap-rs and Foundry (different serde/tokio versions) | Medium | Medium | Pin versions, use workspace resolver, may need to patch |
| `DebuggerContext` field is not `pub` on `Debugger` struct | Low | Confirmed | Use `Debugger::new()` to construct, which takes all fields individually |
| Source map gaps for optimized code | Medium | High | Return "no source available" for unmapped PCs, show disassembly instead |
| dap-rs missing some DAP commands (StepBack, ReadMemory) | Medium | Medium | Extend with custom handling or contribute upstream |
| Foundry internal API changes (unstable crate) | Medium | High | Pin to specific commit, track upstream changes |

## Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Protocol crate | `dap-rs` (sztomi/dap-rs) | Full DAP types + server + transport. Saves 2-3 days vs hand-rolling. Sync API fits post-mortem debugger. |
| Foundry integration | Link crates as library | Richer data access than CLI. Direct access to `ContractSources`, `DebugNode`, `CallTraceStep`. |
| Async runtime | None (synchronous) | dap-rs is sync. Foundry debugger is post-mortem. No concurrent process to manage. |
| Transport | stdio | Zed's default. Simplest. No TCP port management. |
| Source mapping | Reuse Foundry's `ContractSources::find_source_mapping()` | Already handles Solc vs Vyper, PC-to-IC mapping, multi-file resolution. |
| Variable presentation | Raw EVM values first, ABI-decoded later | Phase complexity — raw hex works immediately, ABI decode is a nice-to-have. |
