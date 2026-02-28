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
}
