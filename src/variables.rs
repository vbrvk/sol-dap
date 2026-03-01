use alloy_primitives::hex;
use dap::types::Variable;
use foundry_debugger::DebugNode;
use revm_inspectors::tracing::types::CallTraceStep;

pub fn stack_variables(step: &CallTraceStep) -> Vec<Variable> {
    let Some(stack) = &step.stack else {
        tracing::debug!("stack is None for pc={}", step.pc);
        return Vec::new();
    };
    tracing::debug!("stack has {} items for pc={}", stack.len(), step.pc);

    stack
        .iter()
        .enumerate()
        .map(|(i, val)| Variable {
            name: format!("[{i}]"),
            value: format!("0x{:x}", val),
            type_field: Some("uint256".to_string()),
            variables_reference: 0,
            ..Default::default()
        })
        .collect()
}

pub fn memory_variables(step: &CallTraceStep) -> Vec<Variable> {
    let Some(memory) = &step.memory else {
        return Vec::new();
    };

    let bytes = memory.as_ref();
    bytes
        .chunks(32)
        .enumerate()
        .map(|(i, chunk)| Variable {
            name: format!("0x{:04x}", i * 32),
            value: format!("0x{}", hex::encode(chunk)),
            type_field: Some("bytes32".to_string()),
            variables_reference: 0,
            ..Default::default()
        })
        .collect()
}

pub fn calldata_variables(node: &DebugNode) -> Vec<Variable> {
    let data = node.calldata.as_ref();
    if data.is_empty() {
        return Vec::new();
    }

    let mut vars = Vec::new();
    if data.len() >= 4 {
        vars.push(Variable {
            name: "selector".to_string(),
            value: format!("0x{}", hex::encode(&data[..4])),
            type_field: Some("bytes4".to_string()),
            variables_reference: 0,
            ..Default::default()
        });
    }

    let args = data.get(4..).unwrap_or_default();
    for (i, chunk) in args.chunks(32).enumerate() {
        vars.push(Variable {
            name: format!("arg[{i}]"),
            value: format!("0x{}", hex::encode(chunk)),
            type_field: Some("bytes32".to_string()),
            variables_reference: 0,
            ..Default::default()
        });
    }

    vars
}

pub fn returndata_variables(step: &CallTraceStep) -> Vec<Variable> {
    let data = step.returndata.as_ref();
    if data.is_empty() {
        return Vec::new();
    }

    data.chunks(32)
        .enumerate()
        .map(|(i, chunk)| Variable {
            name: format!("[{i}]"),
            value: format!("0x{}", hex::encode(chunk)),
            type_field: Some("bytes32".to_string()),
            variables_reference: 0,
            ..Default::default()
        })
        .collect()
}

pub fn gas_info_variables(step: &CallTraceStep) -> Vec<Variable> {
    vec![
        Variable {
            name: "pc".to_string(),
            value: format!("{}", step.pc),
            type_field: Some("uint".to_string()),
            variables_reference: 0,
            ..Default::default()
        },
        Variable {
            name: "opcode".to_string(),
            value: step.op.to_string(),
            type_field: Some("string".to_string()),
            variables_reference: 0,
            ..Default::default()
        },
        Variable {
            name: "gas_remaining".to_string(),
            value: format!("{}", step.gas_remaining),
            type_field: Some("uint64".to_string()),
            variables_reference: 0,
            ..Default::default()
        },
        Variable {
            name: "gas_cost".to_string(),
            value: format!("{}", step.gas_cost),
            type_field: Some("uint64".to_string()),
            variables_reference: 0,
            ..Default::default()
        },
    ]
}

/// Build variables from storage layout + storage change tracking.
/// Scans all trace steps up to the current position to build the current storage state,
/// then presents named variables from the storage layout.
pub fn storage_variables(
    debug_arena: &[DebugNode],
    current_node: usize,
    current_step: usize,
    contract_name: &str,
    layout: &crate::launch::StorageLayout,
) -> Vec<Variable> {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    // Build current storage state by replaying SLOAD/SSTORE ops.
    // SSTORE opcode = 0x55, stack at SSTORE: [..., slot, value]
    // We scan all steps up to the current position.
    let mut storage: HashMap<U256, U256> = HashMap::new();

    for (ni, node) in debug_arena.iter().enumerate() {
        let max_step = if ni == current_node { current_step } else if ni < current_node { node.steps.len() } else { break };
        for si in 0..max_step {
            let step = &node.steps[si];
            // SSTORE = 0x55
            if step.op.get() == 0x55 {
                if let Some(stack) = &step.stack {
                    if stack.len() >= 2 {
                        let slot = stack[stack.len() - 1];
                        let value = stack[stack.len() - 2];
                        storage.insert(slot, value);
                    }
                }
            }
        }
    }

    // Map storage slots to named variables using the layout.
    let mut vars: Vec<Variable> = Vec::new();
    for entry in &layout.storage {
        let slot: U256 = entry.slot.parse().unwrap_or_default();
        let value = storage.get(&slot).copied().unwrap_or_default();
        let type_label = layout.types.get(&entry.type_key)
            .map(|t| t.label.as_str())
            .unwrap_or("unknown");

        // Format value based on type
        let formatted = if type_label.starts_with("uint") {
            format!("{value}")  // decimal for uints
        } else if type_label.starts_with("int") {
            // Signed int — interpret as i256
            format!("{value}")  // TODO: proper signed display
        } else if type_label.starts_with("address") || type_label.starts_with("contract") {
            format!("0x{:040x}", value)  // 20-byte address
        } else if type_label == "bool" {
            if value.is_zero() { "false".to_string() } else { "true".to_string() }
        } else {
            format!("0x{:x}", value)
        };

        vars.push(Variable {
            name: entry.label.clone(),
            value: formatted,
            type_field: Some(type_label.to_string()),
            variables_reference: 0,
            ..Default::default()
        });
    }
    vars
}
