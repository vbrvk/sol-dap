use alloy_primitives::map::AddressHashMap;
use alloy_primitives::{U256, hex};
use dap::types::Variable;
use foundry_debugger::DebugNode;
use revm_inspectors::tracing::types::CallTraceStep;
use std::collections::HashMap;

use crate::launch::EventInfo;

/// Format a U256 value based on its Solidity type.
fn format_typed_value(val: &alloy_primitives::U256, type_hint: &str) -> String {
    if type_hint.starts_with("address") || type_hint.starts_with("contract") {
        format!("0x{:040x}", val)
    } else if type_hint == "bool" {
        if val.is_zero() {
            "false".to_string()
        } else {
            "true".to_string()
        }
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
                let word = alloy_primitives::U256::from_be_slice(chunks[i]);
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
        for (i, chunk) in chunks.iter().enumerate().skip(params.len()) {
            vars.push(Variable {
                name: format!("arg[{i}]"),
                value: format!("0x{}", hex::encode(chunk)),
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

pub fn event_variables(
    debug_arena: &[DebugNode],
    current_node: usize,
    current_step: usize,
    event_signatures: &HashMap<String, EventInfo>,
    identified_contracts: &AddressHashMap<String>,
) -> Vec<Variable> {
    let events = collect_events(
        debug_arena,
        current_node,
        current_step,
        event_signatures,
        identified_contracts,
    );

    events
        .into_iter()
        .enumerate()
        .map(|(event_idx, ev)| {
            let addr_short = format!("0x{:x}", ev.address);
            let value = if let Some(info) = &ev.event_info {
                let params = ev
                    .decoded_params
                    .iter()
                    .map(|(n, _t, v)| format!("{n}={v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{}({}).{}({})",
                    ev.contract_name, addr_short, info.name, params
                )
            } else {
                let topics = ev
                    .topics
                    .iter()
                    .map(|t| format!("0x{:064x}", t))
                    .collect::<Vec<_>>()
                    .join(", ");
                let data = hex::encode(&ev.data);
                format!(
                    "{}({}).LOG{}(topics=[{}], data=0x{})",
                    ev.contract_name,
                    addr_short,
                    ev.topics.len(),
                    topics,
                    data
                )
            };

            Variable {
                name: format!("event[{event_idx}]"),
                value,
                type_field: Some("event".to_string()),
                variables_reference: 0,
                ..Default::default()
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct CollectedEvent {
    pub address: alloy_primitives::Address,
    pub contract_name: String,
    pub event_info: Option<EventInfo>,
    pub topics: Vec<alloy_primitives::U256>,
    pub data: Vec<u8>,
    pub decoded_params: Vec<(String, String, String)>,
}

pub fn collect_events(
    debug_arena: &[DebugNode],
    current_node: usize,
    current_step: usize,
    event_signatures: &HashMap<String, EventInfo>,
    identified_contracts: &AddressHashMap<String>,
) -> Vec<CollectedEvent> {
    use alloy_primitives::U256;

    fn u256_to_usize(v: U256) -> Option<usize> {
        if v.bit_len() > usize::BITS as usize {
            return None;
        }
        Some(v.to::<u64>() as usize)
    }

    let mut out = Vec::new();

    for (node_idx, node) in debug_arena.iter().enumerate() {
        if node_idx > current_node {
            break;
        }

        let max_step = if node_idx < current_node {
            node.steps.len()
        } else {
            (current_step + 1).min(node.steps.len())
        };

        for step_idx in 0..max_step {
            let step = &node.steps[step_idx];
            let op = step.op.get();
            if !(0xa0..=0xa4).contains(&op) {
                continue;
            }

            let topic_count = (op - 0xa0) as usize;
            let Some(stack) = &step.stack else {
                continue;
            };
            if stack.len() < 2 + topic_count {
                continue;
            }

            let offset = stack[stack.len() - 1];
            let size = stack[stack.len() - 2];

            let mut topics: Vec<U256> = Vec::new();
            for i in 0..topic_count {
                topics.push(stack[stack.len() - 3 - i]);
            }

            let data = if let (Some(memory), Some(off), Some(sz)) = (
                step.memory.as_ref(),
                u256_to_usize(offset),
                u256_to_usize(size),
            ) {
                let bytes = memory.as_ref();
                let end = off.saturating_add(sz).min(bytes.len());
                if off < bytes.len() {
                    bytes[off..end].to_vec()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let topic0 = topics.first().map(|t0| format!("0x{:064x}", t0));
            let event_info = topic0
                .as_deref()
                .and_then(|t0| event_signatures.get(t0))
                .cloned();

            let contract_name = identified_contracts
                .get(&node.address)
                .cloned()
                .unwrap_or_else(|| format!("0x{:x}", node.address));

            let mut decoded_params: Vec<(String, String, String)> = Vec::new();
            if let Some(ref info) = event_info {
                let mut topic_idx = 1usize;
                let mut data_word_idx = 0usize;
                for (i, p) in info.params.iter().enumerate() {
                    let pname = if p.name.is_empty() {
                        format!("arg[{i}]")
                    } else {
                        p.name.clone()
                    };

                    let val_opt: Option<U256> = if p.indexed {
                        topics.get(topic_idx).copied().inspect(|_v| {
                            topic_idx += 1;
                        })
                    } else {
                        let start = data_word_idx * 32;
                        let end = start + 32;
                        if end <= data.len() {
                            data_word_idx += 1;
                            Some(U256::from_be_slice(&data[start..end]))
                        } else {
                            None
                        }
                    };

                    let formatted = match val_opt {
                        Some(v) => format_typed_value(&v, &p.type_field),
                        None => "(missing)".to_string(),
                    };
                    decoded_params.push((pname, p.type_field.clone(), formatted));
                }
            }

            out.push(CollectedEvent {
                address: node.address,
                contract_name,
                event_info,
                topics,
                data,
                decoded_params,
            });
        }
    }

    out
}

/// Build variables from storage layout + storage change tracking.
/// Scans ALL trace steps (including CREATE nodes) to build storage state.
pub fn storage_variables(
    debug_arena: &[DebugNode],
    current_node: usize,
    _current_step: usize,
    node_address: &alloy_primitives::Address,
    layout: &crate::launch::StorageLayout,
) -> Vec<Variable> {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    // Replay ALL SSTORE ops for this contract address (including constructor).
    let mut storage: HashMap<U256, U256> = HashMap::new();
    for (node_idx, node) in debug_arena.iter().enumerate() {
        if &node.address != node_address {
            continue;
        }
        // For the current node, scan ALL steps (not just up to current_step).
        // This shows storage values that will be written by the current call frame,
        // which is more useful for a post-mortem debugger than the exact mid-opcode state.
        let max_step = if node_idx <= current_node {
            node.steps.len()
        } else {
            continue;
        };
        for step_idx in 0..max_step {
            let step = &node.steps[step_idx];
            if step.op.get() == 0x55
                && let Some(stack) = &step.stack
                && stack.len() >= 2
            {
                storage.insert(stack[stack.len() - 1], stack[stack.len() - 2]);
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
        } else if type_label.starts_with("uint") || type_label.starts_with("int") {
            format!("{value}")
        } else if type_label.starts_with("address") || type_label.starts_with("contract") {
            format!("0x{:040x}", value)
        } else if type_label == "bool" {
            if value.is_zero() {
                "false".to_string()
            } else {
                "true".to_string()
            }
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
        if len == 0 {
            return "\"\"".to_string();
        }
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

const CONSOLE_LOG_ADDRESS_BYTES: [u8; 20] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x63, 0x6f, 0x6e, 0x73, 0x6f, 0x6c, 0x65,
    0x2e, 0x6c, 0x6f, 0x67,
];

fn decode_console_log(calldata: &[u8]) -> String {
    fn u256_to_usize(v: U256) -> Option<usize> {
        if v.bit_len() > usize::BITS as usize {
            return None;
        }
        Some(v.to::<u64>() as usize)
    }

    if calldata.len() < 4 {
        return "(empty)".to_string();
    }
    let args = &calldata[4..];
    if args.is_empty() {
        return "(empty)".to_string();
    }

    let words: Vec<&[u8]> = args.chunks(32).collect();
    let mut consumed = vec![false; words.len()];
    let mut parts: Vec<String> = Vec::new();

    for (i, word) in words.iter().enumerate() {
        if consumed[i] {
            continue;
        }

        if word.len() < 32 {
            parts.push(format!("0x{}", hex::encode(word)));
            consumed[i] = true;
            continue;
        }

        let val = U256::from_be_slice(word);

        if let Some(off) = u256_to_usize(val)
            && off % 32 == 0
            && off + 32 <= args.len()
        {
            let len_word_idx = off / 32;
            if len_word_idx < words.len() && words[len_word_idx].len() == 32 {
                let len_u256 = U256::from_be_slice(words[len_word_idx]);
                if let Some(len) = u256_to_usize(len_u256)
                    && len <= 10_000
                {
                    let data_start = off + 32;
                    let data_end = data_start.saturating_add(len);
                    if data_start <= args.len() && data_end <= args.len() {
                        let bytes = &args[data_start..data_end];
                        if let Ok(s) = std::str::from_utf8(bytes) {
                            parts.push(s.to_string());
                            consumed[i] = true;
                            let data_words = len.div_ceil(32);
                            let total_words = 1 + data_words;
                            for j in len_word_idx..len_word_idx.saturating_add(total_words) {
                                if j < consumed.len() {
                                    consumed[j] = true;
                                }
                            }
                            continue;
                        }
                    }
                }
            }
        }

        if word[..31].iter().all(|&b| b == 0) && (word[31] == 0 || word[31] == 1) {
            parts.push(if word[31] == 0 {
                "false".to_string()
            } else {
                "true".to_string()
            });
            consumed[i] = true;
            continue;
        }

        // Non-string, non-bool value: show as decimal
        parts.push(format!("{val}"));
        consumed[i] = true;
        continue;
    }

    if parts.is_empty() {
        "(empty)".to_string()
    } else {
        parts.join(" ")
    }
}

pub fn collect_console_logs(
    debug_arena: &[DebugNode],
    current_node: usize,
    current_step: usize,
) -> Vec<String> {
    fn u256_to_usize(v: U256) -> Option<usize> {
        if v.bit_len() > usize::BITS as usize {
            return None;
        }
        Some(v.to::<u64>() as usize)
    }

    if debug_arena.is_empty() {
        return Vec::new();
    }

    let mut addr32 = [0u8; 32];
    addr32[12..].copy_from_slice(&CONSOLE_LOG_ADDRESS_BYTES);
    let console_addr_u256 = U256::from_be_bytes(addr32);

    let mut out = Vec::new();

    let end = (current_node + 1).min(debug_arena.len());
    for (node_idx, node) in debug_arena.iter().enumerate().take(end) {
        if node.address.as_slice() == CONSOLE_LOG_ADDRESS_BYTES {
            out.push(decode_console_log(node.calldata.as_ref()));
            continue;
        }

        let max_step = if node_idx < current_node {
            node.steps.len()
        } else {
            (current_step + 1).min(node.steps.len())
        };

        for step_idx in 0..max_step {
            let step = &node.steps[step_idx];
            if step.op.get() != 0xfa {
                continue;
            }

            let Some(stack) = &step.stack else {
                continue;
            };
            if stack.len() < 6 {
                continue;
            }

            let to = stack[stack.len().saturating_sub(2)];
            if to != console_addr_u256 {
                continue;
            }

            let in_offset = stack[stack.len().saturating_sub(3)];
            let in_size = stack[stack.len().saturating_sub(4)];

            let Some(memory) = step.memory.as_ref() else {
                continue;
            };
            let (Some(off), Some(sz)) = (u256_to_usize(in_offset), u256_to_usize(in_size)) else {
                continue;
            };
            let bytes = memory.as_ref();
            let end = off.saturating_add(sz);
            if off > bytes.len() || end > bytes.len() {
                continue;
            }

            out.push(decode_console_log(&bytes[off..end]));
        }
    }

    out
}

pub fn console_log_variables(
    debug_arena: &[DebugNode],
    current_node: usize,
    current_step: usize,
) -> Vec<Variable> {
    collect_console_logs(debug_arena, current_node, current_step)
        .into_iter()
        .enumerate()
        .map(|(i, value)| Variable {
            name: format!("log[{i}]"),
            value,
            type_field: Some("string".to_string()),
            variables_reference: 0,
            ..Default::default()
        })
        .collect()
}

/// Build context variables for a call frame.
pub fn context_variables(
    node: &DebugNode,
    _node_index: usize,
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

    // msg.sender: find the node just before the FIRST occurrence of this contract's address.
    let first_entry = debug_arena.iter().position(|n| n.address == node.address);
    if let Some(first) = first_entry
        && first > 0
    {
        vars.push(Variable {
            name: "msg.sender".to_string(),
            value: format!("0x{:x}", debug_arena[first - 1].address),
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
    for (i, line) in lines.iter().enumerate().skip(func_start) {
        for ch in line.chars() {
            if ch == '{' {
                brace_depth += 1;
            }
            if ch == '}' {
                brace_depth -= 1;
            }
        }
        if brace_depth <= 0 && i > func_start {
            func_end = i;
            break;
        }
    }

    // Parse local variable declarations within the function body
    let mut locals: Vec<Variable> = Vec::new();
    let solidity_types = [
        "uint256", "uint128", "uint64", "uint32", "uint16", "uint8", "uint", "int256", "int128",
        "int64", "int32", "int16", "int8", "int", "address", "bool", "bytes32", "bytes", "string",
        "bytes1", "bytes2", "bytes4", "bytes8", "bytes16", "bytes20",
    ];

    for (i, line) in lines
        .iter()
        .enumerate()
        .take(func_end.min(lines.len().saturating_sub(1)) + 1)
        .skip(func_start + 1)
    {
        let trimmed = line.trim();
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
            if let Some(rest) = trimmed.strip_prefix(sol_type) {
                let rest = rest.trim_start();
                // Handle memory/storage/calldata modifiers
                let rest = rest.strip_prefix("memory ").unwrap_or(rest);
                let rest = rest.strip_prefix("storage ").unwrap_or(rest);
                let rest = rest.strip_prefix("calldata ").unwrap_or(rest);
                // Extract variable name (word before = or ;)
                let var_name: String = rest
                    .chars()
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
            let type_name: String = trimmed
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let rest = trimmed[type_name.len()..].trim_start();
            let var_name: String = rest
                .chars()
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
        let declared_locals: Vec<usize> = locals
            .iter()
            .enumerate()
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    // ── format_typed_value ──────────────────────────────────────────

    #[test]
    fn format_address_value() {
        let val = U256::from(0xdead_beef_u64);
        assert_eq!(
            format_typed_value(&val, "address"),
            "0x00000000000000000000000000000000deadbeef"
        );
    }

    #[test]
    fn format_contract_value_same_as_address() {
        let val = U256::from(0xabcd_u64);
        assert_eq!(
            format_typed_value(&val, "contract Foo"),
            format_typed_value(&val, "address"),
        );
    }

    #[test]
    fn format_bool_true_and_false() {
        assert_eq!(format_typed_value(&U256::ZERO, "bool"), "false");
        assert_eq!(format_typed_value(&U256::from(1u64), "bool"), "true");
        assert_eq!(format_typed_value(&U256::from(42u64), "bool"), "true");
    }

    #[test]
    fn format_uint_decimal() {
        let val = U256::from(12345u64);
        assert_eq!(format_typed_value(&val, "uint256"), "12345");
        assert_eq!(format_typed_value(&val, "uint8"), "12345");
        assert_eq!(format_typed_value(&val, "int256"), "12345");
    }

    #[test]
    fn format_fixed_bytes_hex() {
        let val = U256::from(0xff_u64);
        let out = format_typed_value(&val, "bytes32");
        assert!(out.starts_with("0x"), "expected hex prefix, got: {out}");
    }

    #[test]
    fn format_unknown_type_hex() {
        let val = U256::from(255u64);
        let out = format_typed_value(&val, "tuple");
        assert!(
            out.starts_with("0x"),
            "expected hex prefix for unknown type, got: {out}"
        );
    }

    // ── decode_short_string ────────────────────────────────────────

    #[test]
    fn decode_empty_short_string() {
        // len=0 → last byte = 0 (even), len/2=0
        let raw = U256::ZERO;
        assert_eq!(decode_short_string(&raw), "\"\"");
    }

    #[test]
    fn decode_valid_short_string() {
        // Encode "hello" (5 chars) as Solidity short string.
        // Data: b"hello" padded to 31 bytes, last byte = 5*2 = 10
        let mut bytes = [0u8; 32];
        bytes[0] = b'h';
        bytes[1] = b'e';
        bytes[2] = b'l';
        bytes[3] = b'l';
        bytes[4] = b'o';
        bytes[31] = 10; // length * 2
        let raw = U256::from_be_bytes(bytes);
        assert_eq!(decode_short_string(&raw), "\"hello\"");
    }

    #[test]
    fn decode_long_string_reports_length() {
        // Odd last byte → long string encoding
        let mut bytes = [0u8; 32];
        bytes[31] = 0x41; // odd → long string, length = (0x41-1)/2 = 32
        let raw = U256::from_be_bytes(bytes);
        let result = decode_short_string(&raw);
        assert!(
            result.starts_with("(string,"),
            "expected long string marker, got: {result}"
        );
    }
}
