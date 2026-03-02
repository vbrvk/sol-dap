use std::collections::HashMap;
use std::path::PathBuf;

use alloy_primitives::Address;
use alloy_primitives::map::AddressHashMap;
use foundry_debugger::DebugNode;
use foundry_evm_core::Breakpoints;
use foundry_evm_traces::debug::ContractSources;
use revm_inspectors::tracing::types::CallTraceStep;

use crate::config::LaunchConfig;
use crate::launch::{DebuggerContext, EventInfo, StorageLayout};
use crate::source_map::{self, SourceLocation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Breakpoint,
    End,
}

pub struct DebugSession {
    pub debug_arena: Vec<DebugNode>,
    pub identified_contracts: AddressHashMap<String>,
    pub contracts_sources: ContractSources,
    pub breakpoints: Breakpoints,
    pub current_node: usize,
    pub current_step: usize,
    /// Source breakpoints: file → line numbers
    pub source_breakpoints: HashMap<PathBuf, Vec<i64>>,
    pub launch_config: LaunchConfig,
    /// Contract name → storage layout for variable name resolution
    pub storage_layouts: HashMap<String, StorageLayout>,
    /// Function selector → signature for frame name resolution
    pub method_identifiers: HashMap<String, String>,
    pub event_signatures: HashMap<String, EventInfo>,
    /// Function selector → parameter names/types for stack labeling
    pub function_params: HashMap<String, Vec<(String, String)>>,
    /// Number of console.log messages already emitted to the debug console
    pub last_emitted_log_count: usize,
}

impl DebugSession {
    pub fn new(ctx: DebuggerContext, config: LaunchConfig) -> Self {
        Self {
            debug_arena: ctx.debug_arena,
            identified_contracts: ctx.identified_contracts,
            contracts_sources: ctx.contracts_sources,
            breakpoints: ctx.breakpoints,
            storage_layouts: ctx.storage_layouts,
            method_identifiers: ctx.method_identifiers,
            event_signatures: ctx.event_signatures,
            function_params: ctx.function_params,
            current_node: 0,
            current_step: 0,
            source_breakpoints: HashMap::new(),
            launch_config: config,
            last_emitted_log_count: 0,
        }
    }

    pub fn current_debug_node(&self) -> Option<&DebugNode> {
        self.debug_arena.get(self.current_node)
    }

    pub fn current_trace_step(&self) -> Option<&CallTraceStep> {
        self.current_debug_node()
            .and_then(|node| node.steps.get(self.current_step))
    }

    pub fn current_address(&self) -> Option<&Address> {
        self.current_debug_node().map(|node| &node.address)
    }

    pub fn current_contract_name(&self) -> Option<&str> {
        self.current_address()
            .and_then(|addr| self.identified_contracts.get(addr))
            .map(|s| s.as_str())
    }

    pub fn total_nodes(&self) -> usize {
        self.debug_arena.len()
    }

    pub fn current_node_step_count(&self) -> usize {
        self.current_debug_node()
            .map(|n| n.steps.len())
            .unwrap_or(0)
    }

    pub fn is_at_end(&self) -> bool {
        if self.debug_arena.is_empty() {
            return true;
        }
        self.current_node >= self.debug_arena.len() - 1
            && self.current_step >= self.current_node_step_count().saturating_sub(1)
    }

    pub fn step_opcode(&mut self) {
        if self.debug_arena.is_empty() {
            return;
        }
        if self.is_at_end() {
            return;
        }

        let node_steps = self.current_node_step_count();
        if node_steps == 0 {
            if self.current_node + 1 < self.total_nodes() {
                self.current_node += 1;
                self.current_step = 0;
            }
            return;
        }

        if self.current_step + 1 < node_steps {
            self.current_step += 1;
            return;
        }

        if self.current_node + 1 < self.total_nodes() {
            self.current_node += 1;
            self.current_step = 0;
        }
    }

    pub fn step_back_opcode(&mut self) {
        if self.debug_arena.is_empty() {
            return;
        }
        if self.current_node == 0 && self.current_step == 0 {
            return;
        }

        if self.current_step > 0 {
            self.current_step -= 1;
            return;
        }

        if self.current_node > 0 {
            self.current_node -= 1;
            let prev_steps = self.current_node_step_count();
            self.current_step = prev_steps.saturating_sub(1);
        }
    }

    /// Step Over: advance to the next source line WITHOUT entering child calls.
    /// If the current line triggers a CALL, skip over the entire child execution
    /// and stop when we return to the same (or higher) call depth.
    pub fn step_next(&mut self) {
        let start_node = self.current_node;
        let start_loc = self
            .current_source_location()
            .map(|loc| (loc.path.clone(), loc.line));

        loop {
            if self.is_at_end() {
                break;
            }
            self.step_opcode();

            // If we entered a deeper node (child call), skip forward until we
            // return to start_node or a later node at the same depth.
            // In the flat arena, the "return" from a child is the next occurrence
            // of the parent's address (or a node index > child).
            if self.current_node > start_node {
                // We entered a child call. Skip until we're back at start_node's
                // address or past it.
                self.skip_to_node_return(start_node);
                if self.is_at_end() {
                    break;
                }
            }

            let now = self.current_source_location();
            match (&start_loc, &now) {
                (None, Some(loc)) => {
                    if !self.is_contract_definition_line(loc) && !Self::is_library_code(loc) {
                        break;
                    }
                }
                (Some(a), Some(b)) if a.0 != b.path || a.1 != b.line => {
                    if !self.is_contract_definition_line(b) && !Self::is_library_code(b) {
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    /// Skip forward until we return from a child call back to the parent.
    /// The flat arena has parent nodes interleaved with child nodes:
    ///   Node A (parent), Node B (child), Node C (parent continues), ...
    /// When we're inside Node B, we advance until we find a node with the same
    /// address as Node A (the parent returned).
    fn skip_to_node_return(&mut self, parent_node_idx: usize) {
        let parent_address = self.debug_arena[parent_node_idx].address;
        loop {
            if self.is_at_end() {
                break;
            }
            self.step_opcode();
            // Check if we returned to the parent (same address = same contract)
            // and we're past the original node
            if self.current_node > parent_node_idx
                && self.debug_arena[self.current_node].address == parent_address
            {
                break;
            }
        }
    }

    /// Step Into: advance until entering a new DebugNode (child call).
    /// If the current line doesn't have a CALL, behaves like step_next.
    pub fn step_in(&mut self) {
        let start_node = self.current_node;
        let start_loc = self
            .current_source_location()
            .map(|loc| (loc.path.clone(), loc.line));

        loop {
            if self.is_at_end() {
                break;
            }
            self.step_opcode();

            // If we entered a new node, stop there (stepped into the call)
            if self.current_node != start_node {
                // Skip contract-definition preamble
                let loc = self.current_source_location();
                if let Some(loc) = &loc
                    && (self.is_contract_definition_line(loc) || Self::is_library_code(loc))
                {
                    continue;
                }
                break;
            }

            // If same node but different source line, stop (no call to step into)
            let now = self.current_source_location();
            match (&start_loc, &now) {
                (None, Some(loc)) => {
                    if !self.is_contract_definition_line(loc) && !Self::is_library_code(loc) {
                        break;
                    }
                }
                (Some(a), Some(b)) if a.0 != b.path || a.1 != b.line => {
                    if !self.is_contract_definition_line(b) && !Self::is_library_code(b) {
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    /// Step Out: return to the caller.
    ///
    /// Uses the solc source map's Jump::Out indicator to detect function returns.
    /// For external calls, advances until we leave the current DebugNode.
    /// For internal calls, advances until we find a JUMP with Jump::Out marker,
    /// then steps once more to land on the caller's line.
    pub fn step_out(&mut self) {
        if self.debug_arena.is_empty() {
            return;
        }
        let start_node = self.current_node;

        loop {
            if self.is_at_end() {
                break;
            }

            // Check if the CURRENT step is a function return (Jump::Out)
            // before advancing, so we can detect the return point.
            let is_return = {
                let node = &self.debug_arena[self.current_node];
                let step = &node.steps[self.current_step];
                let contract_name = self
                    .identified_contracts
                    .get(&node.address)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                source_map::is_jump_out(
                    step,
                    contract_name,
                    &self.contracts_sources,
                    node.kind.is_any_create(),
                )
            };

            self.step_opcode();

            if is_return {
                // We just executed a return JUMP. Now skip unmapped/contract-def opcodes
                // to land on the caller's meaningful source line.
                loop {
                    if self.is_at_end() {
                        break;
                    }
                    if let Some(loc) = self.current_source_location()
                        && !self.is_contract_definition_line(&loc)
                        && !Self::is_library_code(&loc)
                    {
                        break;
                    }
                    self.step_opcode();
                }
                break;
            }

            // If we crossed a node boundary (external call return), stop
            if self.current_node > start_node {
                // Skip contract-def lines in the parent
                loop {
                    if self.is_at_end() {
                        break;
                    }
                    if let Some(loc) = self.current_source_location()
                        && !self.is_contract_definition_line(&loc)
                        && !Self::is_library_code(&loc)
                    {
                        break;
                    }
                    self.step_opcode();
                }
                break;
            }
        }
    }

    /// Check if a source location points to a contract definition line.
    /// The solc source map maps the dispatcher preamble to the `contract X {` line.
    fn is_contract_definition_line(&self, loc: &SourceLocation) -> bool {
        let Some(node) = self.current_debug_node() else {
            return false;
        };
        let Some(first_step) = node.steps.first() else {
            return false;
        };
        let contract_name = match self.current_contract_name() {
            Some(n) => n,
            None => return false,
        };
        if let Some(first_loc) = source_map::step_to_source(
            first_step,
            contract_name,
            &self.contracts_sources,
            node.kind.is_any_create(),
            &self.launch_config.project_root,
        ) {
            first_loc.path == loc.path && first_loc.line == loc.line
        } else {
            false
        }
    }

    /// Check if a source location is in a library (forge-std, lib/).
    /// These should be skipped during stepping to stay in user code.
    fn is_library_code(loc: &SourceLocation) -> bool {
        let path_str = loc.path.to_string_lossy();
        path_str.contains("forge-std")
            || path_str.contains("/lib/")
            || path_str.contains("console.sol")
    }

    pub fn continue_to_breakpoint(&mut self) -> StopReason {
        // Record the breakpoint we're currently stopped at (if any).
        // We need to skip ALL occurrences of this breakpoint until we hit a DIFFERENT one.
        let skip_bp = self.current_source_location().and_then(|loc| {
            self.source_breakpoints.get(&loc.path).and_then(|lines| {
                if lines.contains(&loc.line) {
                    Some((loc.path.clone(), loc.line))
                } else {
                    None
                }
            })
        });

        loop {
            if self.is_at_end() {
                return StopReason::End;
            }
            self.step_opcode();

            let loc = match self.current_source_location() {
                Some(l) => l,
                None => continue,
            };
            if Self::is_library_code(&loc) {
                continue;
            }

            // Check if this is a breakpoint
            let is_bp = self
                .source_breakpoints
                .get(&loc.path)
                .is_some_and(|lines| lines.contains(&loc.line));
            if !is_bp {
                continue;
            }

            // Skip if it's the same breakpoint we started on
            if let Some((ref skip_path, skip_line)) = skip_bp
                && &loc.path == skip_path
                && loc.line == skip_line
            {
                continue;
            }

            return StopReason::Breakpoint;
        }
    }

    pub fn current_source_location(&self) -> Option<SourceLocation> {
        let node = self.current_debug_node()?;
        let step = self.current_trace_step()?;
        let contract_name = self.current_contract_name().unwrap_or("Unknown");
        source_map::step_to_source(
            step,
            contract_name,
            &self.contracts_sources,
            node.kind.is_any_create(),
            &self.launch_config.project_root,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_code_detected_forge_std() {
        let loc = SourceLocation {
            path: PathBuf::from("/proj/lib/forge-std/src/Test.sol"),
            line: 10,
            column: 1,
            length: 5,
        };
        assert!(DebugSession::is_library_code(&loc));
    }

    #[test]
    fn library_code_detected_lib_dir() {
        let loc = SourceLocation {
            path: PathBuf::from("/proj/lib/openzeppelin-contracts/ERC20.sol"),
            line: 5,
            column: 1,
            length: 10,
        };
        assert!(DebugSession::is_library_code(&loc));
    }

    #[test]
    fn library_code_detected_console() {
        let loc = SourceLocation {
            path: PathBuf::from("/proj/lib/forge-std/src/console.sol"),
            line: 1,
            column: 1,
            length: 3,
        };
        assert!(DebugSession::is_library_code(&loc));
    }

    #[test]
    fn user_code_not_flagged_as_library() {
        let loc = SourceLocation {
            path: PathBuf::from("/proj/src/Counter.sol"),
            line: 15,
            column: 1,
            length: 8,
        };
        assert!(!DebugSession::is_library_code(&loc));
    }

    #[test]
    fn stop_reason_equality() {
        assert_eq!(StopReason::Breakpoint, StopReason::Breakpoint);
        assert_eq!(StopReason::End, StopReason::End);
        assert_ne!(StopReason::Breakpoint, StopReason::End);
    }
}
