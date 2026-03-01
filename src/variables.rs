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

/// Format EVM stack as DAP Variables.
pub fn stack_variables(step: &CallTraceStep) -> Vec<Variable> {
    let Some(stack) = &step.stack else {
        return Vec::new();
    };
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

/// Format calldata as DAP Variables, decoding params if ABI info available.
pub fn calldata_variables(
    node: &DebugNode,
    function_params: Option<&[(String, String)]>,
    fn_signature: Option<&str>,
) -> Vec<Variable> {
    let data = node.calldata.as_ref();
    if data.is_empty() {
        return Vec::new();
    }

    let mut vars = Vec::new();

    // Function selector + decoded name
    if data.len() >= 4 {
        let sel_hex = format!("0x{}", hex::encode(&data[..4]));
        let label = match fn_signature {
            Some(sig) => format!("{sel_hex} ({})", sig),
            None => sel_hex,
        };
        vars.push(Variable {
            name: "function".to_string(),
            value: label,
            type_field: Some("bytes4".to_string()),
            variables_reference: 0,
            ..Default::default()
        });
    }

    // Decode parameters from calldata using ABI info
    let args_data = data.get(4..).unwrap_or_default();
    let chunks: Vec<&[u8]> = args_data.chunks(32).collect();

    if let Some(params) = function_params {
        // We have ABI param info — decode with names and types
        for (i, param) in params.iter().enumerate() {
            let value = if i < chunks.len() {
                let word = alloy_primitives::U256::from_be_slice(
                    &pad_to_32(chunks[i]),
                );
                format_typed_value(&word, &param.1)
            } else {
                "(missing)".to_string()
            };
            vars.push(Variable {
                name: param.0.clone(),
                value,
                type_field: Some(param.1.clone()),
                variables_reference: 0,
                ..Default::default()
            });
        }
        // Show any extra calldata beyond params
        for i in params.len()..chunks.len() {
            vars.push(Variable {
                name: format!("arg[{i}]"),
                value: format!("0x{}", hex::encode(chunks[i])),
                type_field: Some("bytes32".to_string()),
                variables_reference: 0,
                ..Default::default()
            });
        }
    } else {
        // No ABI info — raw 32-byte chunks
        for (i, chunk) in chunks.iter().enumerate() {
            vars.push(Variable {
                name: format!("arg[{i}]"),
                value: format!("0x{}", hex::encode(chunk)),
                type_field: Some("bytes32".to_string()),
                variables_reference: 0,
                ..Default::default()
            });
        }
    }

    vars
}

/// Pad a slice to 32 bytes (right-pad with zeros).
fn pad_to_32(data: &[u8]) -> [u8; 32] {
    let mut buf = [0u8; 32];
    let len = data.len().min(32);
    buf[32 - len..].copy_from_slice(&data[..len]);
    buf
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
/// Scans ALL trace steps (including CREATE nodes) to build storage state.
pub fn storage_variables(
    debug_arena: &[DebugNode],
    current_node: usize,
    current_step: usize,
    node_address: &alloy_primitives::Address,
    layout: &crate::launch::StorageLayout,
) -> Vec<Variable> {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    // Replay ALL SSTORE ops for this contract address (including constructor).
    let mut storage: HashMap<U256, U256> = HashMap::new();
    for (ni, node) in debug_arena.iter().enumerate() {
        if &node.address != node_address {
            if ni > current_node { break; }
            continue;
        }
        let max_step = if ni == current_node { current_step } else if ni < current_node { node.steps.len() } else { break };
        for si in 0..max_step {
            let step = &node.steps[si];
            if step.op.get() == 0x55 {
                if let Some(stack) = &step.stack {
                    if stack.len() >= 2 {
                        storage.insert(stack[stack.len() - 1], stack[stack.len() - 2]);
                    }
                }
            }
        }
    }

    let mut vars: Vec<Variable> = Vec::new();
    for entry in &layout.storage {
        let slot: U256 = entry.slot.parse().unwrap_or_default();
        let type_info = layout.types.get(&entry.type_key);
        let type_label = type_info.map(|t| t.label.as_str()).unwrap_or("unknown");
        let encoding = type_info.and_then(|t| t.encoding.as_deref()).unwrap_or("");

        if encoding == "mapping" {
            vars.push(Variable {
                name: entry.label.clone(),
                value: format!("({type_label})"),
                type_field: Some(type_label.to_string()),
                variables_reference: 0,
                ..Default::default()
            });
            continue;
        }

        let value = storage.get(&slot).copied().unwrap_or_default();

        let formatted = if encoding == "bytes" || type_label == "string" {
            decode_short_string(&value)
        } else if type_label.starts_with("uint") {
            format!("{value}")
        } else if type_label.starts_with("int") {
            format!("{value}")
        } else if type_label.starts_with("address") || type_label.starts_with("contract") {
            format!("0x{:040x}", value)
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

/// Decode a Solidity short string stored in a single storage slot.
/// Short strings (<32 bytes): data in bytes[0..len], length*2 in last byte.
pub fn decode_short_string(raw: &alloy_primitives::U256) -> String {
    let bytes: [u8; 32] = raw.to_be_bytes();
    let last_byte = bytes[31];
    if last_byte & 1 == 0 {
        let len = (last_byte / 2) as usize;
        if len == 0 { return "\"\"".to_string(); }
        let len = len.min(31);
        match std::str::from_utf8(&bytes[..len]) {
            Ok(s) => format!("\"{s}\""),
            Err(_) => format!("0x{}", hex::encode(&bytes[..len])),
        }
    } else {
        let total = alloy_primitives::U256::from_be_slice(&bytes);
        let len = (total - alloy_primitives::U256::from(1)) / alloy_primitives::U256::from(2);
        format!("(string, {len} bytes)")
    }
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

/// Extract local variable declarations from a Solidity function body.
/// Parses the source file to find the enclosing function and its local
/// variable declarations (e.g., `uint256 fee = ...`, `bool success = ...`).
///
/// Returns variables with their names and types. Values are inferred from
/// the stack when possible.
pub fn local_variables(
    source_path: &std::path::Path,
    current_line: i64,
    step: &CallTraceStep,
) -> Vec<Variable> {
    let source = match std::fs::read_to_string(source_path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let lines: Vec<&str> = source.lines().collect();

    // Find the enclosing function: scan backward from current_line for 'function ...'
    let current_idx = (current_line as usize).saturating_sub(1);
    let mut func_start = current_idx;
    for i in (0..=current_idx.min(lines.len().saturating_sub(1))).rev() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("function ") || trimmed.contains("function ") {
            func_start = i;
            break;
        }
    }

    // Find the function's closing brace by counting braces
    let mut brace_depth = 0i32;
    let mut func_end = current_idx;
    for i in func_start..lines.len() {
        for ch in lines[i].chars() {
            if ch == '{' { brace_depth += 1; }
            if ch == '}' { brace_depth -= 1; }
        }
        if brace_depth <= 0 && i > func_start {
            func_end = i;
            break;
        }
    }

    // Parse local variable declarations within the function body
    let mut locals: Vec<Variable> = Vec::new();
    let solidity_types = [
        "uint256", "uint128", "uint64", "uint32", "uint16", "uint8", "uint",
        "int256", "int128", "int64", "int32", "int16", "int8", "int",
        "address", "bool", "bytes32", "bytes", "string",
        "bytes1", "bytes2", "bytes4", "bytes8", "bytes16", "bytes20",
    ];

    for i in (func_start + 1)..=func_end.min(lines.len().saturating_sub(1)) {
        let trimmed = lines[i].trim();
        // Skip empty lines, comments, control flow
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with('{')
            || trimmed.starts_with('}')
            || trimmed.starts_with("require")
            || trimmed.starts_with("if ")
            || trimmed.starts_with("for ")
            || trimmed.starts_with("while ")
            || trimmed.starts_with("emit ")
            || trimmed.starts_with("revert ")
            || trimmed.starts_with("return")
        {
            continue;
        }

        // Check for type-prefixed declarations: 'uint256 fee = ...' or 'bool success = ...'
        for sol_type in &solidity_types {
            if trimmed.starts_with(sol_type) {
                let rest = trimmed[sol_type.len()..].trim_start();
                // Handle memory/storage/calldata modifiers
                let rest = rest.strip_prefix("memory ").unwrap_or(rest);
                let rest = rest.strip_prefix("storage ").unwrap_or(rest);
                let rest = rest.strip_prefix("calldata ").unwrap_or(rest);
                // Extract variable name (word before = or ;)
                let var_name: String = rest.chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !var_name.is_empty() {
                    let declared = i < current_idx; // only show if line is before current
                    let line_num = i + 1;
                    locals.push(Variable {
                        name: var_name,
                        value: if declared {
                            format!("(declared at line {line_num})")
                        } else {
                            "(not yet declared)".to_string()
                        },
                        type_field: Some(sol_type.to_string()),
                        variables_reference: 0,
                        ..Default::default()
                    });
                }
                break;
            }
        }

        // Also handle contract/interface type declarations: 'SimpleToken token = ...'
        // These start with an uppercase letter
        if trimmed.chars().next().is_some_and(|c| c.is_uppercase()) {
            let type_name: String = trimmed.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let rest = trimmed[type_name.len()..].trim_start();
            let var_name: String = rest.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !var_name.is_empty() && !var_name.starts_with(char::is_uppercase) {
                let declared = i < current_idx;
                let line_num = i + 1;
                locals.push(Variable {
                    name: var_name,
                    value: if declared {
                        format!("(declared at line {line_num})")
                    } else {
                        "(not yet declared)".to_string()
                    },
                    type_field: Some(type_name),
                    variables_reference: 0,
                    ..Default::default()
                });
            }
        }
    }

    // Try to assign values from the stack for the most recently declared local.
    // The topmost stack value often corresponds to the result of the previous expression.
    // This is a best-effort heuristic.
    if let Some(stack) = &step.stack {
        // Assign stack values top-down to the most recently declared locals
        let declared_locals: Vec<usize> = locals.iter().enumerate()
            .filter(|(_, v)| v.value.starts_with("(declared"))
            .map(|(i, _)| i)
            .rev()
            .collect();
        for (stack_idx, &local_idx) in declared_locals.iter().enumerate() {
            if stack_idx < stack.len() {
                let val = stack[stack.len() - 1 - stack_idx];
                let type_hint = locals[local_idx].type_field.as_deref().unwrap_or("");
                locals[local_idx].value = format_typed_value(&val, type_hint);
            }
        }
    }

    locals
}
