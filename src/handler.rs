use alloy_primitives::hex;

use std::io::{Read, Write};
use std::path::PathBuf;

use dap::prelude::*;

use crate::config::LaunchConfig;
use crate::session::{DebugSession, StopReason};
use crate::{source_map, variables};


/// Evaluate an expression in the debug console.
fn evaluate_expression(
    expr: &str,
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> String {
    match expr {
        // === EVM execution context ===
        "pc" => step.pc.to_string(),
        "op" | "opcode" => step.op.to_string(),
        "gas" => step.gas_remaining.to_string(),
        "gas_cost" => step.gas_cost.to_string(),
        "depth" | "node" => session.current_node.to_string(),
        "step" => session.current_step.to_string(),

        // === Addresses ===
        "address" | "this" => session
            .current_address()
            .map(|a| format!("0x{:x}", a))
            .unwrap_or_else(|| "N/A".to_string()),
        "msg.sender" | "caller" => {
            if session.current_node > 0 {
                format!("0x{:x}", session.debug_arena[session.current_node - 1].address)
            } else {
                "N/A (top-level call)".to_string()
            }
        }

        // === Memory ===
        "memory" | "memory.length" => step
            .memory
            .as_ref()
            .map(|m| format!("{} bytes", m.len()))
            .unwrap_or_else(|| "not available".to_string()),

        // === Calldata / returndata ===
        "calldata" | "msg.data" => session
            .current_debug_node()
            .map(|n| format!("0x{}", hex::encode(&n.calldata)))
            .unwrap_or_default(),
        "returndata" => format!("0x{}", hex::encode(&step.returndata)),

        // === Stack ===
        "stack" => {
            match &step.stack {
                Some(stack) => {
                    if stack.is_empty() {
                        "[] (empty)".to_string()
                    } else {
                        let items: Vec<String> = stack.iter().enumerate().rev()
                            .map(|(i, v)| format!("  [{}] 0x{:x}", i, v))
                            .collect();
                        format!("Stack ({} items):\n{}", stack.len(), items.join("\n"))
                    }
                }
                None => "not available".to_string(),
            }
        }

        // === Mapping lookups: _balances[addr], _allowances[from][spender] ===
        _ if expr.contains('[') && expr.contains(']') => {
            eval_mapping_or_storage(expr, step, session)
        }

        // === Storage variables (by Solidity name) ===
        _ if is_storage_variable(expr, session) => {
            eval_storage_variable(expr, session)
        }

        // stack[N] and memory[N] are handled by eval_mapping_or_storage above

        // === Help ===
        "help" | "?" => {
            "Available expressions:\n\
              pc, op, gas, gas_cost, depth, step\n\
              address, this, msg.sender, caller\n\
              stack, stack[N]\n\
              memory, memory[offset], memory[offset:length]\n\
              calldata, msg.data, returndata\n\
              <variable_name> (storage variables, e.g. 'number')\n\
              <mapping>[<key>] (e.g. '_balances[msg.sender]', '_allowances[from][spender]')\n\
              keys can be: hex (0x...), decimal, this, msg.sender, calldata param names\n\
              help, ?"
            .to_string()
        }

        _ => format!("unknown expression: '{}'. Type 'help' for available expressions.", expr),
    }
}

/// Check if an expression matches a storage variable name in any contract.
fn is_storage_variable(name: &str, session: &DebugSession) -> bool {
    for layout in session.storage_layouts.values() {
        if layout.storage.iter().any(|e| e.label == name) {
            return true;
        }
    }
    false
}

/// Evaluate a storage variable by name — replays SSTORE ops to get current value.
fn eval_storage_variable(name: &str, session: &DebugSession) -> String {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    let mut slot_info: Option<(U256, &str, &str, &str)> = None;  // slot, type_label, encoding, contract_name
    for (contract_name, layout) in &session.storage_layouts {
        for entry in &layout.storage {
            if entry.label == name {
                let slot: U256 = entry.slot.parse().unwrap_or_default();
                let type_label = layout.types.get(&entry.type_key)
                    .map(|t| t.label.as_str()).unwrap_or("unknown");
                let encoding = layout.types.get(&entry.type_key)
                    .and_then(|t| t.encoding.as_deref()).unwrap_or("");
                slot_info = Some((slot, type_label, encoding, contract_name.as_str()));
                break;
            }
        }
    }

    let Some((slot, type_label, encoding, owner_name)) = slot_info else {
        return format!("variable '{}' not found in storage layout", name);
    };

    if encoding == "mapping" {
        return format!("{name} ({type_label})");
    }

    // Replay SSTORE for the owning contract's address
    let target_address = session.identified_contracts.iter()
        .find(|(_, v)| v.as_str() == owner_name)
        .map(|(addr, _)| *addr)
        .or_else(|| session.current_address().cloned());
    let mut storage: HashMap<U256, U256> = HashMap::new();
    for (ni, node) in session.debug_arena.iter().enumerate() {
        if target_address.as_ref() != Some(&node.address) { continue; }
        let max_step = if ni <= session.current_node { node.steps.len() } else { continue };
        for si in 0..max_step {
            let s = &node.steps[si];
            if s.op.get() == 0x55 {
                if let Some(stack) = &s.stack {
                    if stack.len() >= 2 {
                        storage.insert(stack[stack.len() - 1], stack[stack.len() - 2]);
                    }
                }
            }
        }
    }

    let value = storage.get(&slot).copied().unwrap_or_default();

    let formatted = if encoding == "bytes" || type_label == "string" {
        variables::decode_short_string(&value)
    } else if type_label.starts_with("uint") {
        format!("{value}")
    } else if type_label.starts_with("address") || type_label.starts_with("contract") {
        format!("0x{:040x}", value)
    } else if type_label == "bool" {
        if value.is_zero() { "false".to_string() } else { "true".to_string() }
    } else {
        format!("0x{:x}", value)
    };

    format!("{name} ({type_label}) = {formatted}")
}

/// Evaluate mapping lookups like `_balances[0xaddr]` or `_balances[from]`.
/// Also handles nested mappings like `_allowances[from][spender]`.
/// Falls back to stack[N] evaluation if the name isn't a storage mapping.
fn eval_mapping_or_storage(
    expr: &str,
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> String {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    // Parse name[key] or name[key1][key2]
    let bracket_pos = match expr.find('[') {
        Some(p) => p,
        None => return format!("invalid expression: {expr}"),
    };
    let var_name = &expr[..bracket_pos];

    // Check if it's a known storage mapping — search ALL contracts' layouts
    let mut mapping_slot: Option<U256> = None;
    let mut value_type: Option<String> = None;
    let mut owner_contract: Option<String> = None;
    for (contract_name, layout) in &session.storage_layouts {
        for entry in &layout.storage {
            if entry.label == var_name {
                let encoding = layout.types.get(&entry.type_key)
                    .and_then(|t| t.encoding.as_deref()).unwrap_or("");
                if encoding == "mapping" {
                    mapping_slot = Some(entry.slot.parse().unwrap_or_default());
                    let vtype = layout.types.get(&entry.type_key)
                        .and_then(|t| t.value_type.as_deref());
                    if let Some(vt) = vtype {
                        value_type = layout.types.get(vt).map(|t| t.label.clone());
                    }
                    owner_contract = Some(contract_name.clone());
                }
                break;
            }
        }
    }

    let Some(base_slot) = mapping_slot else {
        // Not a mapping — fall back to stack[N] if it looks like that
        if var_name == "stack" {
            return eval_stack_index(&expr[6..expr.len()-1], step);
        } else if var_name == "memory" {
            return eval_memory_slice(&expr[7..expr.len()-1], step);
        }
        return format!("'{var_name}' is not a storage mapping");
    };

    // Parse all [key] segments
    let mut keys: Vec<&str> = Vec::new();
    let mut rest = &expr[bracket_pos..];
    while rest.starts_with('[') {
        let end = match rest.find(']') {
            Some(p) => p,
            None => return format!("unclosed bracket in: {expr}"),
        };
        keys.push(&rest[1..end]);
        rest = &rest[end+1..];
    }

    if keys.is_empty() {
        return format!("no key provided for mapping {var_name}");
    }

    // Resolve keys to U256 values
    let resolved_keys: Vec<U256> = keys.iter().map(|key| {
        resolve_value(key.trim(), step, session)
    }).collect();

    // Compute the storage slot: keccak256(abi.encode(key, slot))
    // For nested mappings: keccak256(abi.encode(key2, keccak256(abi.encode(key1, slot))))
    let mut current_slot = base_slot;
    for key in &resolved_keys {
        let mut data = [0u8; 64];
        key.to_be_bytes::<32>().iter().enumerate().for_each(|(i, &b)| data[i] = b);
        current_slot.to_be_bytes::<32>().iter().enumerate().for_each(|(i, &b)| data[32 + i] = b);
        current_slot = U256::from_be_bytes(alloy_primitives::keccak256(&data).0);
    }

    // Find ALL contract addresses that have this storage variable.
    // This handles inheritance: VaultWithCallback inherits Vault's deposits mapping.
    // We scan all addresses that match, plus the current address as fallback.
    let mut target_addresses: Vec<alloy_primitives::Address> = Vec::new();

    // 1. Find the exact owner contract's address
    if let Some(name) = &owner_contract {
        for (addr, cname) in &session.identified_contracts {
            if cname.as_str() == name {
                target_addresses.push(*addr);
            }
        }
    }

    // 2. Also include the current address (handles inheritance)
    if let Some(addr) = session.current_address() {
        if !target_addresses.contains(addr) {
            target_addresses.push(*addr);
        }
    }

    // 3. Also include any contract that has this variable in its layout
    for (cname, layout) in &session.storage_layouts {
        if layout.storage.iter().any(|e| e.label == var_name) {
            for (addr, identified_name) in &session.identified_contracts {
                if identified_name.as_str() == cname && !target_addresses.contains(addr) {
                    target_addresses.push(*addr);
                }
            }
        }
    }

    let mut storage: HashMap<U256, U256> = HashMap::new();
    for (ni, node) in session.debug_arena.iter().enumerate() {
        if !target_addresses.contains(&node.address) { continue; }
        let max_step = if ni <= session.current_node {
            node.steps.len()
        } else {
            continue
        };
        for si in 0..max_step {
            let s = &node.steps[si];
            if s.op.get() == 0x55 {
                if let Some(stack) = &s.stack {
                    if stack.len() >= 2 {
                        storage.insert(stack[stack.len() - 1], stack[stack.len() - 2]);
                    }
                }
            }
        }
    }

    let value = storage.get(&current_slot).copied().unwrap_or_default();

    // Format based on value type
    let type_label = value_type.as_deref().unwrap_or("uint256");
    let formatted = if type_label.starts_with("uint") {
        format!("{value}")
    } else if type_label.starts_with("address") || type_label.starts_with("contract") {
        format!("0x{:040x}", value)
    } else if type_label == "bool" {
        if value.is_zero() { "false".to_string() } else { "true".to_string() }
    } else {
        format!("0x{:x}", value)
    };

    let keys_str = keys.join("][");
    format!("{var_name}[{keys_str}] ({type_label}) = {formatted}")
}

/// Resolve a value expression to a U256.
/// Handles: hex literals (0x...), decimal numbers, "this"/"address",
/// "msg.sender"/"caller", and evaluating other storage variables.
fn resolve_value(
    expr: &str,
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> alloy_primitives::U256 {
    use alloy_primitives::U256;

    let expr = expr.trim();

    // Hex literal
    if let Some(hex_str) = expr.strip_prefix("0x") {
        return U256::from_str_radix(hex_str, 16).unwrap_or_default();
    }

    // Decimal literal
    if let Ok(n) = expr.parse::<u128>() {
        return U256::from(n);
    }

    // this / address
    if expr == "this" || expr == "address" {
        return session.current_address()
            .map(|a| U256::from_be_slice(a.as_slice()))
            .unwrap_or_default();
    }

    // msg.sender / caller — find the actual calling contract by scanning backward
    // for the first node with a different address (the one that made the CALL)
    if expr == "msg.sender" || expr == "caller" {
        let current_addr = session.current_address().cloned();
        for ni in (0..session.current_node).rev() {
            if Some(&session.debug_arena[ni].address) != current_addr.as_ref() {
                return U256::from_be_slice(session.debug_arena[ni].address.as_slice());
            }
        }
        return U256::ZERO;
    }

    // Stack reference: stack[N]
    if let Some(idx_str) = expr.strip_prefix("stack[").and_then(|s| s.strip_suffix(']')) {
        if let Ok(idx) = idx_str.parse::<usize>() {
            if let Some(stack) = &step.stack {
                if idx < stack.len() {
                    return stack[stack.len() - 1 - idx];
                }
            }
        }
        return U256::ZERO;
    }

    // Try as a storage variable name (e.g., 'from' if it's a named param in calldata)
    // Actually, local variable names on the stack aren't reliably mapped.
    // But we can try calldata params: look for the name in function_params.
    if let Some(node) = session.current_debug_node() {
        if node.calldata.len() >= 4 {
            let sel = format!("0x{}", alloy_primitives::hex::encode(&node.calldata[..4]));
            if let Some(params) = session.function_params.get(&sel) {
                for (i, (pname, _ptype)) in params.iter().enumerate() {
                    if pname == expr {
                        // Decode from calldata: params are at offset 4 + i*32
                        let offset = 4 + i * 32;
                        if offset + 32 <= node.calldata.len() {
                            return U256::from_be_slice(&node.calldata[offset..offset + 32]);
                        }
                    }
                }
            }
        }
    }

    U256::ZERO
}

fn eval_stack_index(idx_str: &str, step: &revm_inspectors::tracing::types::CallTraceStep) -> String {
    match idx_str.parse::<usize>() {
        Ok(idx) => {
            if let Some(stack) = &step.stack {
                if idx < stack.len() {
                    format!("0x{:x}", stack[stack.len() - 1 - idx])
                } else {
                    format!("stack index {idx} out of bounds (stack has {} items)", stack.len())
                }
            } else {
                "stack not available".to_string()
            }
        }
        Err(_) => "invalid stack index".to_string(),
    }
}

/// Evaluate memory[offset] or memory[offset:length]
fn eval_memory_slice(inner: &str, step: &revm_inspectors::tracing::types::CallTraceStep) -> String {
    let memory = match &step.memory {
        Some(m) => m.as_ref(),
        None => return "memory not available".to_string(),
    };

    let (offset, length) = if let Some((off_s, len_s)) = inner.split_once(':') {
        let off = off_s.trim().parse::<usize>().unwrap_or(0);
        let len = len_s.trim().parse::<usize>().unwrap_or(32);
        (off, len)
    } else {
        let off = inner.trim().parse::<usize>().unwrap_or(0);
        (off, 32) // default 32 bytes (one word)
    };

    if offset >= memory.len() {
        return format!("offset {} out of bounds (memory is {} bytes)", offset, memory.len());
    }
    let end = (offset + length).min(memory.len());
    format!("0x{}", hex::encode(&memory[offset..end]))
}

const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(BASE64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(BASE64_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(BASE64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(BASE64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Emit console.log output when stepping past a source line containing console.log.
fn emit_console_logs<R: Read, W: Write>(
    server: &mut dap::server::Server<R, W>,
    session: &mut Option<DebugSession>,
) {
    let Some(session) = session.as_mut() else { return };
    let Some(loc) = session.current_source_location() else { return };

    // Skip forge-std internals
    let path_str = loc.path.to_string_lossy();
    if path_str.contains("forge-std") || path_str.contains("console.sol") {
        return;
    }

    // Deduplicate: don't emit again for the same position
    let pos = (loc.path.clone(), loc.line);
    if session.last_console_log_pos.as_ref() == Some(&pos) {
        return;
    }

    // Check if current source line contains console.log
    let source = match std::fs::read_to_string(&loc.path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let line_idx = (loc.line as usize).saturating_sub(1);
    let Some(source_line) = source.lines().nth(line_idx) else { return };
    if !source_line.contains("console.log") && !source_line.contains("console2.log") {
        return;
    }

    // Emit the next pending log
    if session.next_console_log_idx < session.console_logs.len() {
        let msg = session.console_logs[session.next_console_log_idx].clone();
        let _ = server.send_event(Event::Output(events::OutputEventBody {
            category: Some(types::OutputEventCategory::Console),
            output: format!("{msg}\n"),
            data: None,
            source: None,
            line: None,
            column: None,
            variables_reference: None,
            group: None,
        }));
        session.next_console_log_idx += 1;
        session.last_console_log_pos = Some(pos);
    }
}

/// Find the first step in a node that maps to a meaningful source location
/// (i.e., not the contract definition line which is the dispatcher preamble).
/// Returns the step index, falling back to 0 if nothing better is found.
fn find_meaningful_step(
    node: &foundry_debugger::DebugNode,
    contract_name: &str,
    sources: &foundry_evm_traces::debug::ContractSources,
    project_root: &std::path::Path,
) -> usize {
    let is_create = node.kind.is_any_create();
    // Get the source location of step 0 (the dispatcher)
    let first_loc = node.steps.first().and_then(|s| {
        source_map::step_to_source(s, contract_name, sources, is_create, project_root)
    });
    let first_line = first_loc.as_ref().map(|l| (&l.path, l.line));

    // Scan forward for a step that maps to a DIFFERENT source line
    for (i, step) in node.steps.iter().enumerate().skip(1) {
        if let Some(loc) = source_map::step_to_source(step, contract_name, sources, is_create, project_root) {
            match &first_line {
                Some((first_path, first_ln)) => {
                    if &loc.path != *first_path || loc.line != *first_ln {
                        return i;
                    }
                }
                None => return i, // first was unmapped, this one is mapped
            }
        }
        // Don't scan too far — the dispatcher is usually <50 opcodes
        if i > 100 {
            break;
        }
    }
    0 // fallback to first step
}

fn emit_stopped<R: Read, W: Write>(
    server: &mut dap::server::Server<R, W>,
    session: &mut Option<DebugSession>,
    reason: types::StoppedEventReason,
    description: Option<String>,
) {
    let _ = server.send_event(Event::Stopped(events::StoppedEventBody {
        reason,
        description,
        thread_id: Some(1),
        preserve_focus_hint: None,
        text: None,
        all_threads_stopped: None,
        hit_breakpoint_ids: None,
    }));
    emit_memory_event(server, session);
    emit_console_logs(server, session);
}

/// Emit a memory event indicating the EVM memory has been updated.
/// The memory reference "evm-memory" is a virtual reference for the entire EVM memory space.
fn emit_memory_event<R: Read, W: Write>(
    server: &mut dap::server::Server<R, W>,
    session: &Option<DebugSession>,
) {
    if let Some(session) = session.as_ref() {
        if let Some(step) = session.current_trace_step() {
            let mem_size = step.memory.as_ref().map(|m| m.len()).unwrap_or(0);
            let _ = server.send_event(Event::Memory(events::MemoryEventBody {
                memory_reference: "evm-memory".to_string(),
                offset: 0,
                count: mem_size as i64,
            }));
        }
    }
}

pub fn handle_request<R: Read, W: Write>(
    req: &dap::requests::Request,
    server: &mut dap::server::Server<R, W>,
    session: &mut Option<DebugSession>,
    last_config: &Option<crate::config::LaunchConfig>,
) -> dap::responses::Response {
    match &req.command {
        Command::Initialize(_) => {
            let capabilities = types::Capabilities {
                supports_configuration_done_request: Some(true),
                supports_function_breakpoints: Some(false),
                supports_step_back: Some(true),
                supports_terminate_request: Some(true),
                supports_restart_request: Some(true),
                supports_evaluate_for_hovers: Some(true),
                supports_read_memory_request: Some(true),
                supports_memory_event: Some(true),

                ..Default::default()
            };

            // Note: Initialized event must be sent AFTER the response.
            // This is handled in main.rs after server.respond().

            req.clone().success(ResponseBody::Initialize(capabilities))
        }
        Command::ConfigurationDone(_) => req.clone().success(ResponseBody::ConfigurationDone),
        Command::Disconnect(_) => req.clone().success(ResponseBody::Disconnect),
        Command::Launch(args) => {
            tracing::info!("launch args: {:?}", args.additional_data);
            let config = match args.additional_data.as_ref() {
                Some(data) => match LaunchConfig::from_args(data) {
                    Ok(c) => c,
                    Err(e) => return req.clone().error(&format!("invalid launch config: {e:#}")),
                },
                None => return req.clone().error("missing launch configuration"),
            };

            match crate::launch::compile_and_debug(&config) {
                Ok(ctx) => {
                    *session = Some(DebugSession::new(ctx, config));

                    emit_stopped(server, session, types::StoppedEventReason::Entry, None);

                    req.clone().success(ResponseBody::Launch)
                }
                Err(e) => req.clone().error(&format!("launch failed: {e:#}")),
            }
        }
        Command::Continue(_) => {
            let stop_reason = match session.as_mut() {
                Some(s) => s.continue_to_breakpoint(),
                None => return req.clone().error("no active debug session"),
            };

            match stop_reason {
                StopReason::Breakpoint => {
                    emit_stopped(server, session, types::StoppedEventReason::Breakpoint, None);
                }
                StopReason::End => {
                    emit_stopped(
                        server,
                        session,
                        types::StoppedEventReason::Step,
                        Some("end of trace".to_string()),
                    );
                }
            }

            let body = responses::ContinueResponse {
                all_threads_continued: Some(true),
            };
            req.clone().success(ResponseBody::Continue(body))
        }
        Command::Next(_) => {
            match session.as_mut() {
                Some(s) => s.step_next(),
                None => return req.clone().error("no active debug session"),
            };
            emit_stopped(server, session, types::StoppedEventReason::Step, None);
            req.clone().success(ResponseBody::Next)
        }
        Command::StepIn(_) => {
            match session.as_mut() {
                Some(s) => s.step_in(),
                None => return req.clone().error("no active debug session"),
            };
            emit_stopped(server, session, types::StoppedEventReason::Step, None);
            req.clone().success(ResponseBody::StepIn)
        }
        Command::StepOut(_) => {
            match session.as_mut() {
                Some(s) => s.step_out(),
                None => return req.clone().error("no active debug session"),
            };
            emit_stopped(server, session, types::StoppedEventReason::Step, None);
            req.clone().success(ResponseBody::StepOut)
        }
        Command::Pause(_) => {
            emit_stopped(server, session, types::StoppedEventReason::Pause, None);
            req.clone().success(ResponseBody::Pause)
        }
        Command::StepBack(_) => {
            match session.as_mut() {
                Some(s) => s.step_back_opcode(),
                None => return req.clone().error("no active debug session"),
            };
            emit_stopped(server, session, types::StoppedEventReason::Step, None);
            req.clone().success(ResponseBody::StepBack)
        }
        Command::Threads(_) => {
            let body = responses::ThreadsResponse {
                threads: vec![types::Thread {
                    id: 1,
                    name: "EVM Execution".to_string(),
                }],
            };

            req.clone().success(ResponseBody::Threads(body))
        }
        Command::StackTrace(args) => {
            let session = match session.as_ref() {
                Some(s) => s,
                None => return req.clone().error("no active debug session"),
            };

            if args.thread_id != 1 {
                return req.clone().error("unknown thread_id");
            }

            let mut frames: Vec<types::StackFrame> = Vec::new();
            for (i, node) in session
                .debug_arena
                .iter()
                .enumerate()
                .take(session.current_node.saturating_add(1))
            {
                let contract_name = session
                    .identified_contracts
                    .get(&node.address)
                    .map(|s| s.as_str())
                    .unwrap_or("Unknown");

                // For the current frame, use the current step.
                // For parent frames, find the first step with a meaningful source location
                // (skip the dispatcher preamble that maps to the contract definition line).
                let step_idx = if i == session.current_node {
                    session.current_step
                } else {
                    find_meaningful_step(node, contract_name, &session.contracts_sources, &session.launch_config.project_root)
                };
                let Some(step) = node.steps.get(step_idx) else {
                    continue;
                };

                // Resolve function name from calldata selector.
                // The first 4 bytes of calldata are the function selector.
                let fn_name = if node.calldata.len() >= 4 {
                    let selector = format!("0x{}", alloy_primitives::hex::encode(&node.calldata[..4]));
                    session.method_identifiers.get(&selector).cloned()
                } else {
                    None
                };
                let frame_name = match &fn_name {
                    Some(sig) => format!("{}::{}", contract_name, sig),
                    None => format!("{}::{}", contract_name, node.kind),
                };
                let mut frame = types::StackFrame {
                    id: i as i64,
                    name: frame_name,
                    line: 0,
                    column: 0,
                    ..Default::default()
                };

                tracing::debug!(
                    "source_map: contract={}, pc={}, is_create={}, node_kind={:?}",
                    contract_name, step.pc, node.kind.is_any_create(), node.kind
                );
                if let Some(loc) = source_map::step_to_source(
                    step,
                    contract_name,
                    &session.contracts_sources,
                    node.kind.is_any_create(),
                    &session.launch_config.project_root,
                ) {
                    // Use path relative to project_root for display
                    let display_path = loc.path
                        .strip_prefix(&session.launch_config.project_root)
                        .unwrap_or(&loc.path);
                    frame.source = Some(types::Source {
                        path: Some(loc.path.to_string_lossy().to_string()),
                        name: Some(display_path.to_string_lossy().to_string()),
                        ..Default::default()
                    });
                    frame.line = loc.line;
                    frame.column = loc.column;

                }

                frames.push(frame);
            }

            frames.reverse();

            let start = args.start_frame.unwrap_or(0).max(0) as usize;
            let levels = args.levels.unwrap_or(0);
            let total = frames.len();

            let frames = if start >= total {
                Vec::new()
            } else if levels <= 0 {
                frames.into_iter().skip(start).collect()
            } else {
                frames
                    .into_iter()
                    .skip(start)
                    .take(levels as usize)
                    .collect()
            };

            let body = responses::StackTraceResponse {
                stack_frames: frames,
                total_frames: None,
            };

            req.clone().success(ResponseBody::StackTrace(body))
        }
        Command::Scopes(args) => {
            let _session = match session.as_ref() {
                Some(s) => s,
                None => return req.clone().error("no active debug session"),
            };

            let frame_id = args.frame_id;

            let scopes = vec![
                types::Scope {
                    name: "Locals".to_string(),
                    variables_reference: 8000 + frame_id,
                    expensive: false,
                    ..Default::default()
                },
                types::Scope {
                    name: "Stack".to_string(),
                    variables_reference: 1000 + frame_id,
                    expensive: false,
                    ..Default::default()
                },
                types::Scope {
                    name: "Memory".to_string(),
                    variables_reference: 2000 + frame_id,
                    expensive: false,
                    ..Default::default()
                },
                types::Scope {
                    name: "Calldata".to_string(),
                    variables_reference: 3000 + frame_id,
                    expensive: false,
                    ..Default::default()
                },
                types::Scope {
                    name: "Return Data".to_string(),
                    variables_reference: 4000 + frame_id,
                    expensive: false,
                    ..Default::default()
                },
                types::Scope {
                    name: "Gas Info".to_string(),
                    variables_reference: 5000 + frame_id,
                    expensive: false,
                    ..Default::default()
                },
                types::Scope {
                    name: "Storage Variables".to_string(),
                    variables_reference: 6000 + frame_id,
                    expensive: false,
                    ..Default::default()
                },
                types::Scope {
                    name: "Context".to_string(),
                    variables_reference: 7000 + frame_id,
                    expensive: false,
                    ..Default::default()
                },
            ];

            let body = responses::ScopesResponse { scopes };
            req.clone().success(ResponseBody::Scopes(body))
        }
        Command::Variables(args) => {
            let session = match session.as_ref() {
                Some(s) => s,
                None => return req.clone().error("no active debug session"),
            };

            let var_ref = args.variables_reference;
            if var_ref <= 0 {
                let body = responses::VariablesResponse {
                    variables: Vec::new(),
                };
                return req.clone().success(ResponseBody::Variables(body));
            }

            let scope_type = var_ref / 1000;
            let frame_id = (var_ref % 1000) as i64;
            let Ok(frame_idx) = usize::try_from(frame_id) else {
                let body = responses::VariablesResponse {
                    variables: Vec::new(),
                };
                return req.clone().success(ResponseBody::Variables(body));
            };

            let node = session.debug_arena.get(frame_idx);
            let step = node.and_then(|n| {
                let step_idx = if frame_idx == session.current_node {
                    session.current_step
                } else {
                    0
                };
                n.steps.get(step_idx)
            });

            let mut vars = match (scope_type, node, step) {
                (1, _, Some(step)) => variables::stack_variables(step),
                (2, _, Some(step)) => variables::memory_variables(step),
                (3, Some(node), _) => {
                    // Decode calldata with ABI param names if available
                    let (fn_params, fn_sig) = if node.calldata.len() >= 4 {
                        let sel = format!("0x{}", alloy_primitives::hex::encode(&node.calldata[..4]));
                        (
                            session.function_params.get(&sel).map(|v| v.as_slice()),
                            session.method_identifiers.get(&sel).map(|s| s.as_str()),
                        )
                    } else {
                        (None, None)
                    };
                    variables::calldata_variables(node, fn_params, fn_sig)
                }
                (4, _, Some(step)) => variables::returndata_variables(step),
                (5, _, Some(step)) => variables::gas_info_variables(step),
                (6, Some(node), _) => {
                    let contract_name = session
                        .identified_contracts
                        .get(&node.address)
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    if let Some(layout) = session.storage_layouts.get(contract_name) {
                        variables::storage_variables(
                            &session.debug_arena,
                            session.current_node,
                            session.current_step,
                            &node.address,
                            layout,
                        )
                    } else {
                        Vec::new()
                    }
                }
                (7, Some(node), step) => {
                    variables::context_variables(node, frame_idx, &session.debug_arena, step)
                }
                (8, _, Some(step)) => {
                    // Locals: parse source file for local variable declarations
                    if let Some(loc) = source_map::step_to_source(
                        step,
                        session.identified_contracts.get(&session.debug_arena[frame_idx].address)
                            .map(|s| s.as_str()).unwrap_or(""),
                        &session.contracts_sources,
                        session.debug_arena[frame_idx].kind.is_any_create(),
                        &session.launch_config.project_root,
                    ) {
                        variables::local_variables(&loc.path, loc.line, step)
                    } else {
                        Vec::new()
                    }
                }
                _ => Vec::new(),
            };

            let start = args.start.unwrap_or(0).max(0) as usize;
            let count = args.count.unwrap_or(0);
            if start < vars.len() {
                if count > 0 {
                    vars = vars.into_iter().skip(start).take(count as usize).collect();
                } else {
                    vars = vars.into_iter().skip(start).collect();
                }
            } else {
                vars.clear();
            }

            let body = responses::VariablesResponse { variables: vars };
            req.clone().success(ResponseBody::Variables(body))
        }
        Command::SetBreakpoints(args) => {
            let session = match session.as_mut() {
                Some(s) => s,
                None => return req.clone().error("no active debug session"),
            };

            let source_path = args.source.path.as_ref().map(PathBuf::from);
            let Some(source_path) = source_path else {
                let body = responses::SetBreakpointsResponse {
                    breakpoints: Vec::new(),
                };
                return req.clone().success(ResponseBody::SetBreakpoints(body));
            };

            let requested_lines: Vec<i64> = args
                .breakpoints
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|bp| bp.line)
                .collect();

            session
                .source_breakpoints
                .insert(source_path.clone(), requested_lines.clone());

            let body = responses::SetBreakpointsResponse {
                breakpoints: requested_lines
                    .into_iter()
                    .map(|line| types::Breakpoint {
                        verified: true,
                        line: Some(line),
                        ..Default::default()
                    })
                    .collect(),
            };

            req.clone().success(ResponseBody::SetBreakpoints(body))
        }
        Command::Evaluate(args) => {
            let session = match session.as_ref() {
                Some(s) => s,
                None => return req.clone().error("no active debug session"),
            };

            let Some(step) = session.current_trace_step() else {
                return req
                    .clone()
                    .success(ResponseBody::Evaluate(responses::EvaluateResponse {
                        result: "not available".to_string(),
                        ..Default::default()
                    }));
            };

            let expr = args.expression.trim();
            let result = evaluate_expression(expr, step, session);

            let body = responses::EvaluateResponse {
                result,
                type_field: None,
                presentation_hint: None,
                variables_reference: 0,
                named_variables: None,
                indexed_variables: None,
                memory_reference: None,
            };
            req.clone().success(ResponseBody::Evaluate(body))
        }
        Command::Restart(_) => {
            let config = session.as_ref()
                .map(|s| s.launch_config.clone())
                .or_else(|| last_config.clone());
            let config = match config {
                Some(c) => c,
                None => return req.clone().error("no launch config available for restart"),
            };

            match crate::launch::compile_and_debug(&config) {
                Ok(ctx) => {
                    *session = Some(DebugSession::new(ctx, config));
                    emit_stopped(server, session, types::StoppedEventReason::Entry, None);
                    req.clone().success(ResponseBody::Restart)
                }
                Err(e) => req.clone().error(&format!("restart failed: {e:#}")),
            }
        }
        Command::Terminate(_) => {
            *session = None;
            let _ = server.send_event(Event::Terminated(Some(events::TerminatedEventBody {
                restart: Some(serde_json::Value::Bool(false)),
            })));
            req.clone().success(ResponseBody::Terminate)
        }
        Command::ReadMemory(args) => {
            let session = match session.as_ref() {
                Some(s) => s,
                None => return req.clone().error("no active debug session"),
            };

            let step = match session.current_trace_step() {
                Some(s) => s,
                None => {
                    return req.clone().success(ResponseBody::ReadMemory(
                        responses::ReadMemoryResponse {
                            address: "0x0".to_string(),
                            unreadable_bytes: None,
                            data: None,
                        },
                    ));
                }
            };

            let memory = step.memory.as_ref().map(|m| m.as_ref()).unwrap_or(&[]);
            let offset = args.offset.unwrap_or(0).max(0) as usize;
            let count = args.count.max(0) as usize;
            let end = (offset + count).min(memory.len());
            let slice = if offset < memory.len() {
                &memory[offset..end]
            } else {
                &[]
            };

            let data = if slice.is_empty() {
                None
            } else {
                // DAP spec: data is base64-encoded.
                // Simple base64 encoding without pulling in another crate.
                
                // Use hex encoding as a fallback readable format.
                // Zed's memory viewer can handle this.
                Some(base64_encode(slice))
            };

            req.clone().success(ResponseBody::ReadMemory(
                responses::ReadMemoryResponse {
                    address: format!("0x{:x}", offset),
                    unreadable_bytes: if end < offset + count {
                        Some((offset + count - end) as i64)
                    } else {
                        None
                    },
                    data,
                },
            ))
        }
        _ => req.clone().error("command not supported"),
    }
}
