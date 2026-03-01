use alloy_primitives::hex;
use dap::types::Variable;
use foundry_debugger::DebugNode;
use revm_inspectors::tracing::types::CallTraceStep;

/// Format a U256 value based on its Solidity type.
fn format_typed_value(val: &alloy_primitives::U256, type_hint: &str) -> String {
    if type_hint.starts_with("address") || type_hint.starts_with("contract") {
        format!("0x{:040x}", val)
    } else if type_hint == "bool" {
        if val.is_zero() { "false".to_string() } else { "true".to_string() }
    } else if type_hint.starts_with("bytes") && type_hint != "bytes" {
        format!("0x{:x}", val)
    } else if type_hint.starts_with("uint") || type_hint.starts_with("int") {
        format!("{}", val)
    } else {
        format!("0x{:x}", val)
    }
}

/// Format EVM stack as DAP Variables, using ABI param names when available.
pub fn stack_variables(
    step: &CallTraceStep,
    function_params: Option<&[(String, String)]>,
) -> Vec<Variable> {
    let Some(stack) = &step.stack else {
        return Vec::new();
    };

    stack
        .iter()
        .enumerate()
        .map(|(i, val)| {
            // Try to name this stack slot from function ABI params.
            // In the EVM, function params are pushed to the stack in reverse order
            // at the start of a function. The bottom of the stack (index 0, 1, ...)
            // corresponds to the function parameters.
            let (name, type_hint) = if let Some(params) = function_params {
                if i < params.len() {
                    (params[i].0.clone(), Some(params[i].1.clone()))
                } else {
                    (format!("[{i}]"), None)
                }
            } else {
                (format!("[{i}]"), None)
            };

            // Format value based on type hint
            let value = if let Some(ref t) = type_hint {
                format_typed_value(val, t)
            } else {
                format!("0x{:x}", val)
            };

            Variable {
                name,
                value,
                type_field: type_hint.or(Some("uint256".to_string())),
                variables_reference: 0,
                ..Default::default()
            }
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

/// Build context variables for a call frame.
pub fn context_variables(
    node: &DebugNode,
    node_index: usize,
    debug_arena: &[DebugNode],
    step: Option<&CallTraceStep>,
) -> Vec<Variable> {
    let mut vars = Vec::new();

    // EVM execution point
    if let Some(step) = step {
        vars.push(Variable {
            name: "pc".to_string(),
            value: format!("{}", step.pc),
            type_field: Some("uint".to_string()),
            variables_reference: 0,
            ..Default::default()
        });
        vars.push(Variable {
            name: "opcode".to_string(),
            value: step.op.to_string(),
            type_field: Some("string".to_string()),
            variables_reference: 0,
            ..Default::default()
        });
    }

    // Addresses
    vars.push(Variable {
        name: "this".to_string(),
        value: format!("0x{:x}", node.address),
        type_field: Some("address".to_string()),
        variables_reference: 0,
        ..Default::default()
    });

    if node_index > 0 {
        let caller = &debug_arena[node_index - 1];
        vars.push(Variable {
            name: "msg.sender".to_string(),
            value: format!("0x{:x}", caller.address),
            type_field: Some("address".to_string()),
            variables_reference: 0,
            ..Default::default()
        });
    }

    vars
}
