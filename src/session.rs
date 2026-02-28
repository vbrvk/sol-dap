use std::collections::HashMap;
use std::path::PathBuf;

use alloy_primitives::map::AddressHashMap;
use alloy_primitives::Address;
use foundry_debugger::DebugNode;
use foundry_evm_core::Breakpoints;
use foundry_evm_traces::debug::ContractSources;
use revm_inspectors::tracing::types::CallTraceStep;

use crate::config::LaunchConfig;
use crate::launch::DebuggerContext;
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
}

impl DebugSession {
    pub fn new(ctx: DebuggerContext, config: LaunchConfig) -> Self {
        Self {
            debug_arena: ctx.debug_arena,
            identified_contracts: ctx.identified_contracts,
            contracts_sources: ctx.contracts_sources,
            breakpoints: ctx.breakpoints,
            current_node: 0,
            current_step: 0,
            source_breakpoints: HashMap::new(),
            launch_config: config,
        }
    }

    pub fn current_debug_node(&self) -> &DebugNode {
        &self.debug_arena[self.current_node]
    }

    pub fn current_trace_step(&self) -> &CallTraceStep {
        &self.current_debug_node().steps[self.current_step]
    }

    pub fn current_address(&self) -> &Address {
        &self.current_debug_node().address
    }

    pub fn current_contract_name(&self) -> Option<&str> {
        self.identified_contracts
            .get(self.current_address())
            .map(|s| s.as_str())
    }

    pub fn total_nodes(&self) -> usize {
        self.debug_arena.len()
    }

    pub fn current_node_step_count(&self) -> usize {
        self.current_debug_node().steps.len()
    }

    pub fn is_at_end(&self) -> bool {
        self.current_node >= self.debug_arena.len() - 1
            && self.current_step >= self.current_node_step_count() - 1
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

    pub fn step_next(&mut self) {
        let start = self
            .current_source_location()
            .map(|loc| (loc.path.clone(), loc.line));

        loop {
            if self.is_at_end() {
                break;
            }
            self.step_opcode();
            let now = self
                .current_source_location()
                .map(|loc| (loc.path.clone(), loc.line));

            match (&start, &now) {
                (None, Some(_)) => break,
                (Some(a), Some(b)) if a != b => break,
                (Some(_), None) => break,
                _ => {}
            }
        }
    }

    pub fn step_in(&mut self) {
        let start_node = self.current_node;
        loop {
            if self.is_at_end() {
                break;
            }
            self.step_opcode();
            if self.current_node != start_node {
                break;
            }
        }
    }

    pub fn step_out(&mut self) {
        if self.debug_arena.is_empty() {
            return;
        }
        let last = self.current_node_step_count().saturating_sub(1);
        self.current_step = last;
    }

    pub fn continue_to_breakpoint(&mut self) -> StopReason {
        loop {
            if self.is_at_end() {
                return StopReason::End;
            }

            self.step_opcode();

            if let Some(loc) = self.current_source_location() {
                if self
                    .source_breakpoints
                    .get(&loc.path)
                    .is_some_and(|lines| lines.iter().any(|&l| l == loc.line))
                {
                    return StopReason::Breakpoint;
                }
            }
        }
    }

    pub fn current_source_location(&self) -> Option<SourceLocation> {
        let node = self.current_debug_node();
        let contract_name = self.current_contract_name().unwrap_or("Unknown");
        source_map::step_to_source(
            self.current_trace_step(),
            contract_name,
            &self.contracts_sources,
            node.kind.is_any_create(),
        )
    }
}
