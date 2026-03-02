use alloy_primitives::hex;

use crate::expr_parser::{self, BinOp, Expr};
use crate::session::DebugSession;
use crate::variables;

/// Evaluate an expression in the debug console.
pub fn evaluate_expression(
    expr: &str,
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> String {
    let expr = expr.trim();

    // Parse into AST
    let ast = match expr_parser::parse(expr) {
        Ok(ast) => ast,
        Err(e) => return format!("parse error: {e}"),
    };

    eval_ast(&ast, step, session)
}

/// Evaluate a parsed AST node, returning a display string.
fn eval_ast(
    ast: &Expr,
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> String {
    match ast {
        Expr::Keyword(kw) => eval_keyword(kw, step, session),
        Expr::HexLiteral(s) => eval_hex_literal(s),
        Expr::DecLiteral(s) => {
            let val = alloy_primitives::U256::from_str_radix(s, 10).unwrap_or_default();
            format_u256_result(val)
        }
        Expr::StackIndex(idx) => eval_stack_index(*idx, step),
        Expr::LogAccess(idx) => eval_log_index(*idx, session),
        Expr::EventAccess { index, field_index } => {
            eval_event_access(*index, *field_index, session)
        }
        Expr::EventField { index, field } => eval_event_field(*index, field, session),
        Expr::MemoryAccess { offset, length } => eval_memory_access(*offset, *length, step),
        Expr::Ident(name) => eval_ident(name, session),
        Expr::MappingLookup { name, keys } => eval_mapping_lookup(name, keys, step, session),
        Expr::BinaryOp { lhs, op, rhs } => {
            let lhs_val = eval_to_u256(lhs, step, session);
            let rhs_val = eval_to_u256(rhs, step, session);
            let result = apply_binop(lhs_val, *op, rhs_val);
            match result {
                Ok(val) => format_u256_result(val),
                Err(e) => e,
            }
        }
    }
}

/// Evaluate an AST node to a U256 value (for use in arithmetic).
fn eval_to_u256(
    ast: &Expr,
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> alloy_primitives::U256 {
    use alloy_primitives::U256;

    match ast {
        Expr::HexLiteral(s) => {
            let hex_str = s
                .strip_prefix("0x")
                .or_else(|| s.strip_prefix("0X"))
                .unwrap_or(s);
            U256::from_str_radix(hex_str, 16).unwrap_or_default()
        }
        Expr::DecLiteral(s) => U256::from_str_radix(s, 10).unwrap_or_default(),
        Expr::StackIndex(idx) => {
            let idx = *idx as usize;
            step.stack
                .as_ref()
                .and_then(|s| {
                    if idx < s.len() {
                        Some(s[s.len() - 1 - idx])
                    } else {
                        None
                    }
                })
                .unwrap_or_default()
        }
        Expr::LogAccess(_) => U256::ZERO,
        Expr::Keyword(kw) => resolve_keyword_to_u256(kw, step, session),
        Expr::Ident(name) => resolve_ident_to_u256(name, step, session),
        Expr::MappingLookup { name, keys } => resolve_mapping_to_u256(name, keys, step, session),
        Expr::BinaryOp { lhs, op, rhs } => {
            let l = eval_to_u256(lhs, step, session);
            let r = eval_to_u256(rhs, step, session);
            apply_binop(l, *op, r).unwrap_or_default()
        }
        Expr::EventAccess { .. } | Expr::EventField { .. } => U256::ZERO,
        Expr::MemoryAccess { offset, .. } => {
            // Read 32 bytes from memory at offset as a U256
            let off = *offset as usize;
            step.memory
                .as_ref()
                .and_then(|m| {
                    let mem = m.as_ref();
                    if off + 32 <= mem.len() {
                        Some(U256::from_be_slice(&mem[off..off + 32]))
                    } else {
                        None
                    }
                })
                .unwrap_or_default()
        }
    }
}

/// Apply a binary operator to two U256 values.
fn apply_binop(
    lhs: alloy_primitives::U256,
    op: BinOp,
    rhs: alloy_primitives::U256,
) -> Result<alloy_primitives::U256, String> {
    Ok(match op {
        BinOp::Add => lhs.wrapping_add(rhs),
        BinOp::Sub => lhs.wrapping_sub(rhs),
        BinOp::Mul => lhs.wrapping_mul(rhs),
        BinOp::Div => {
            if rhs.is_zero() {
                return Err("division by zero".to_string());
            }
            lhs.wrapping_div(rhs)
        }
        BinOp::Mod => {
            if rhs.is_zero() {
                return Err("modulo by zero".to_string());
            }
            lhs.wrapping_rem(rhs)
        }
        BinOp::BitAnd => lhs & rhs,
        BinOp::BitOr => lhs | rhs,
        BinOp::BitXor => lhs ^ rhs,
        BinOp::Shl => {
            let shift: u32 = rhs.try_into().unwrap_or(u32::MAX).min(255);
            lhs << shift as usize
        }
        BinOp::Shr => {
            let shift: u32 = rhs.try_into().unwrap_or(u32::MAX).min(255);
            lhs >> shift as usize
        }
    })
}

// ============ Display-oriented evaluators ============

fn eval_keyword(
    kw: &str,
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> String {
    match kw {
        "pc" => format!("{} (0x{:x})", step.pc, step.pc),
        "op" | "opcode" => step.op.to_string(),
        "gas" => format!("{} (0x{:x})", step.gas_remaining, step.gas_remaining),
        "gas_cost" => format!("{} (0x{:x})", step.gas_cost, step.gas_cost),
        "depth" | "node" => session.current_node.to_string(),
        "step" => session.current_step.to_string(),
        "address" | "this" => session
            .current_address()
            .map(|a| format!("0x{:x}", a))
            .unwrap_or_else(|| "N/A".to_string()),
        "msg.sender" | "caller" => {
            let current_addr = session.current_address().cloned();
            let first_entry = session
                .debug_arena
                .iter()
                .position(|n| Some(&n.address) == current_addr.as_ref());
            if let Some(first) = first_entry {
                if first > 0 {
                    format!("0x{:x}", session.debug_arena[first - 1].address)
                } else {
                    "N/A (top-level call)".to_string()
                }
            } else {
                "N/A".to_string()
            }
        }
        "memory" | "memory.length" => step
            .memory
            .as_ref()
            .map(|m| format!("{} bytes", m.len()))
            .unwrap_or_else(|| "not available".to_string()),
        "calldata" | "msg.data" => session
            .current_debug_node()
            .map(|n| format!("0x{}", hex::encode(&n.calldata)))
            .unwrap_or_default(),
        "returndata" => format!("0x{}", hex::encode(&step.returndata)),
        "stack" => match &step.stack {
            Some(stack) => {
                if stack.is_empty() {
                    "[] (empty)".to_string()
                } else {
                    let items: Vec<String> = stack
                        .iter()
                        .enumerate()
                        .rev()
                        .map(|(i, v)| format!("  [{}] {} (0x{:x})", i, v, v))
                        .collect();
                    format!("Stack ({} items):\n{}", stack.len(), items.join("\n"))
                }
            }
            None => "not available".to_string(),
        },
        "log" => {
            let logs = variables::collect_console_logs(
                &session.debug_arena,
                session.current_node,
                session.current_step,
            );
            if logs.is_empty() {
                "[] (no console logs)".to_string()
            } else {
                let items: Vec<String> = logs
                    .iter()
                    .enumerate()
                    .map(|(i, v)| format!("  log[{i}] {v}"))
                    .collect();
                format!("Console Logs ({}):\n{}", logs.len(), items.join("\n"))
            }
        }
        "help" | "?" => "Available expressions:\n\
              pc, op, gas, gas_cost, depth, step\n\
              address, this, msg.sender, caller\n\
              stack, stack[N]\n\
              log, log[N]\n\
              memory, memory[offset], memory[offset:length]\n\
              calldata, msg.data, returndata\n\
              <variable_name> (storage variables, e.g. 'number')\n\
              <mapping>[<key>] (e.g. '_balances[msg.sender]')\n\
              0xff (hex to decimal conversion)\n\
              <expr> <op> <expr> (arithmetic: +, -, *, /, %, &, |, ^, <<, >>)\n\
              examples: 1 + 1, 0xff - 32, stack[4] << 8, 0xff & fee\n\
              help, ?"
            .to_string(),
        _ => format!("unknown keyword: {kw}"),
    }
}

fn eval_log_index(idx: u64, session: &DebugSession) -> String {
    let logs = variables::collect_console_logs(
        &session.debug_arena,
        session.current_node,
        session.current_step,
    );
    let i: usize = match idx.try_into() {
        Ok(v) => v,
        Err(_) => return "log index too large".to_string(),
    };
    logs.get(i)
        .cloned()
        .unwrap_or_else(|| format!("log[{idx}] out of range ({} logs)", logs.len()))
}

fn resolve_keyword_to_u256(
    kw: &str,
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> alloy_primitives::U256 {
    use alloy_primitives::U256;
    match kw {
        "pc" => U256::from(step.pc),
        "gas" => U256::from(step.gas_remaining),
        "gas_cost" => U256::from(step.gas_cost),
        "depth" | "node" => U256::from(session.current_node),
        "step" => U256::from(session.current_step),
        "address" | "this" => session
            .current_address()
            .map(|a| U256::from_be_slice(a.as_slice()))
            .unwrap_or_default(),
        "msg.sender" | "caller" => {
            let current_addr = session.current_address().cloned();
            let first_entry = session
                .debug_arena
                .iter()
                .position(|n| Some(&n.address) == current_addr.as_ref());
            if let Some(first) = first_entry
                && first > 0
            {
                U256::from_be_slice(session.debug_arena[first - 1].address.as_slice())
            } else {
                U256::ZERO
            }
        }
        _ => U256::ZERO,
    }
}

fn eval_hex_literal(s: &str) -> String {
    use alloy_primitives::U256;
    let hex_str = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    match U256::from_str_radix(hex_str, 16) {
        Ok(val) => format_u256_result(val),
        Err(_) => format!("invalid hex literal: {s}"),
    }
}

fn eval_stack_index(idx: u64, step: &revm_inspectors::tracing::types::CallTraceStep) -> String {
    let idx = idx as usize;
    if let Some(stack) = &step.stack {
        if idx < stack.len() {
            let val = stack[stack.len() - 1 - idx];
            format_u256_result(val)
        } else {
            format!(
                "stack index {idx} out of bounds (stack has {} items)",
                stack.len()
            )
        }
    } else {
        "stack not available".to_string()
    }
}

fn format_collected_event(ev: &variables::CollectedEvent) -> String {
    if let Some(info) = &ev.event_info {
        let params = ev
            .decoded_params
            .iter()
            .map(|(n, _t, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}.{}({})", ev.contract_name, info.name, params)
    } else {
        let topics = ev
            .topics
            .iter()
            .map(|t| format!("0x{:064x}", t))
            .collect::<Vec<_>>()
            .join(", ");
        let data = hex::encode(&ev.data);
        format!(
            "{}.LOG{}(topics=[{}], data=0x{})",
            ev.contract_name,
            ev.topics.len(),
            topics,
            data
        )
    }
}

fn eval_event_access(index: u64, field_index: Option<u64>, session: &DebugSession) -> String {
    let idx = index as usize;
    let events = variables::collect_events(
        &session.debug_arena,
        session.current_node,
        session.current_step,
        &session.event_signatures,
        &session.identified_contracts,
    );

    let Some(ev) = events.get(idx) else {
        return format!("event[{index}] out of bounds ({} events)", events.len());
    };

    if let Some(field_index) = field_index {
        let mi = field_index as usize;
        return ev
            .decoded_params
            .get(mi)
            .map(|(_n, _t, v)| v.clone())
            .unwrap_or_else(|| {
                format!(
                    "event[{index}][{field_index}] out of bounds ({} params)",
                    ev.decoded_params.len()
                )
            });
    }

    format_collected_event(ev)
}

fn eval_event_field(index: u64, field: &str, session: &DebugSession) -> String {
    let idx = index as usize;
    let events = variables::collect_events(
        &session.debug_arena,
        session.current_node,
        session.current_step,
        &session.event_signatures,
        &session.identified_contracts,
    );

    let Some(ev) = events.get(idx) else {
        return format!("event[{index}] out of bounds ({} events)", events.len());
    };

    ev.decoded_params
        .iter()
        .find(|(n, _t, _v)| n == field)
        .map(|(_n, _t, v)| v.clone())
        .unwrap_or_else(|| format!("event[{index}].{field} not found"))
}

fn eval_memory_access(
    offset: u64,
    length: Option<u64>,
    step: &revm_inspectors::tracing::types::CallTraceStep,
) -> String {
    let memory = match &step.memory {
        Some(m) => m.as_ref(),
        None => return "memory not available".to_string(),
    };
    let off = offset as usize;
    let len = length.map(|l| l as usize).unwrap_or(32);
    if off >= memory.len() {
        return format!(
            "offset {} out of bounds (memory is {} bytes)",
            off,
            memory.len()
        );
    }
    let end = (off + len).min(memory.len());
    format!("0x{}", hex::encode(&memory[off..end]))
}

/// Evaluate an identifier (storage variable name or calldata param).
fn eval_ident(name: &str, session: &DebugSession) -> String {
    // Check storage variable
    if is_storage_variable(name, session) {
        return eval_storage_variable(name, session);
    }
    format!("unknown expression: '{name}'. Type 'help' for available expressions.")
}

/// Resolve an identifier to a U256 (for arithmetic).
fn resolve_ident_to_u256(
    name: &str,
    _step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> alloy_primitives::U256 {
    use alloy_primitives::U256;

    // Calldata param
    if let Some(node) = session.current_debug_node()
        && node.calldata.len() >= 4
    {
        let sel = format!("0x{}", alloy_primitives::hex::encode(&node.calldata[..4]));
        if let Some(params) = session.function_params.get(&sel) {
            for (i, (pname, _ptype)) in params.iter().enumerate() {
                if pname == name {
                    let offset = 4 + i * 32;
                    if offset + 32 <= node.calldata.len() {
                        return U256::from_be_slice(&node.calldata[offset..offset + 32]);
                    }
                }
            }
        }
    }

    // Storage variable
    if is_storage_variable(name, session)
        && let Some(val) = resolve_storage_value(name, session)
    {
        return val;
    }

    U256::ZERO
}

// ============ Mapping evaluation ============

/// Display-oriented mapping lookup.
fn eval_mapping_lookup(
    name: &str,
    keys: &[Expr],
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> String {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    // Check if it's a known storage mapping
    let mut mapping_slot: Option<U256> = None;
    let mut value_type: Option<String> = None;
    let mut owner_contract: Option<String> = None;
    for (contract_name, layout) in &session.storage_layouts {
        for entry in &layout.storage {
            if entry.label == name {
                let encoding = layout
                    .types
                    .get(&entry.type_key)
                    .and_then(|t| t.encoding.as_deref())
                    .unwrap_or("");
                if encoding == "mapping" {
                    mapping_slot = Some(entry.slot.parse().unwrap_or_default());
                    let vtype = layout
                        .types
                        .get(&entry.type_key)
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
        return format!("'{name}' is not a storage mapping");
    };

    // Resolve keys
    let resolved_keys: Vec<U256> = keys
        .iter()
        .map(|k| eval_to_u256(k, step, session))
        .collect();

    // Compute keccak256 slots
    let mut current_slot = base_slot;
    for key in &resolved_keys {
        let mut data = [0u8; 64];
        data[..32].copy_from_slice(&key.to_be_bytes::<32>());
        data[32..].copy_from_slice(&current_slot.to_be_bytes::<32>());
        current_slot = U256::from_be_bytes(alloy_primitives::keccak256(data).0);
    }

    // Find target addresses (handles inheritance)
    let mut target_addresses: Vec<alloy_primitives::Address> = Vec::new();
    if let Some(ref owner) = owner_contract {
        for (addr, cname) in &session.identified_contracts {
            if cname.as_str() == owner {
                target_addresses.push(*addr);
            }
        }
    }
    if let Some(addr) = session.current_address()
        && !target_addresses.contains(addr)
    {
        target_addresses.push(*addr);
    }
    for (cname, layout) in &session.storage_layouts {
        if layout.storage.iter().any(|e| e.label == name) {
            for (addr, identified_name) in &session.identified_contracts {
                if identified_name.as_str() == cname && !target_addresses.contains(addr) {
                    target_addresses.push(*addr);
                }
            }
        }
    }

    // Replay SSTORE
    let mut storage: HashMap<U256, U256> = HashMap::new();
    for (ni, node) in session.debug_arena.iter().enumerate() {
        if !target_addresses.contains(&node.address) {
            continue;
        }
        let max_step = if ni <= session.current_node {
            node.steps.len()
        } else {
            continue;
        };
        for si in 0..max_step {
            let s = &node.steps[si];
            if s.op.get() == 0x55
                && let Some(stack) = &s.stack
                && stack.len() >= 2
            {
                storage.insert(stack[stack.len() - 1], stack[stack.len() - 2]);
            }
        }
    }

    let value = storage.get(&current_slot).copied().unwrap_or_default();
    let type_label = value_type.as_deref().unwrap_or("uint256");
    let formatted = format_value_by_type(value, type_label);

    // Build keys display
    let keys_display: Vec<String> = keys.iter().map(|k| format!("{k:?}")).collect();
    // Use a simpler display: just show the original-ish format
    let _ = keys_display;
    format!("{name}[..] ({type_label}) = {formatted}")
}

/// Resolve a mapping lookup to a raw U256 (for arithmetic).
fn resolve_mapping_to_u256(
    name: &str,
    keys: &[Expr],
    step: &revm_inspectors::tracing::types::CallTraceStep,
    session: &DebugSession,
) -> alloy_primitives::U256 {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    let mut mapping_slot: Option<U256> = None;
    let mut owner_contract: Option<String> = None;
    for (contract_name, layout) in &session.storage_layouts {
        for entry in &layout.storage {
            if entry.label == name {
                let encoding = layout
                    .types
                    .get(&entry.type_key)
                    .and_then(|t| t.encoding.as_deref())
                    .unwrap_or("");
                if encoding == "mapping" {
                    mapping_slot = Some(entry.slot.parse().unwrap_or_default());
                    owner_contract = Some(contract_name.clone());
                }
                break;
            }
        }
    }

    let Some(base_slot) = mapping_slot else {
        return U256::ZERO;
    };

    let resolved_keys: Vec<U256> = keys
        .iter()
        .map(|k| eval_to_u256(k, step, session))
        .collect();

    let mut current_slot = base_slot;
    for key in &resolved_keys {
        let mut data = [0u8; 64];
        data[..32].copy_from_slice(&key.to_be_bytes::<32>());
        data[32..].copy_from_slice(&current_slot.to_be_bytes::<32>());
        current_slot = U256::from_be_bytes(alloy_primitives::keccak256(data).0);
    }

    let mut target_addresses: Vec<alloy_primitives::Address> = Vec::new();
    if let Some(ref owner) = owner_contract {
        for (addr, cname) in &session.identified_contracts {
            if cname.as_str() == owner {
                target_addresses.push(*addr);
            }
        }
    }
    if let Some(addr) = session.current_address()
        && !target_addresses.contains(addr)
    {
        target_addresses.push(*addr);
    }
    for (cname, layout) in &session.storage_layouts {
        if layout.storage.iter().any(|e| e.label == name) {
            for (addr, identified_name) in &session.identified_contracts {
                if identified_name.as_str() == cname && !target_addresses.contains(addr) {
                    target_addresses.push(*addr);
                }
            }
        }
    }

    let mut storage: HashMap<U256, U256> = HashMap::new();
    for (ni, node) in session.debug_arena.iter().enumerate() {
        if !target_addresses.contains(&node.address) {
            continue;
        }
        let max_step = if ni <= session.current_node {
            node.steps.len()
        } else {
            continue;
        };
        for si in 0..max_step {
            let s = &node.steps[si];
            if s.op.get() == 0x55
                && let Some(stack) = &s.stack
                && stack.len() >= 2
            {
                storage.insert(stack[stack.len() - 1], stack[stack.len() - 2]);
            }
        }
    }

    storage.get(&current_slot).copied().unwrap_or_default()
}

// ============ Storage helpers ============

fn is_storage_variable(name: &str, session: &DebugSession) -> bool {
    for layout in session.storage_layouts.values() {
        if layout.storage.iter().any(|e| e.label == name) {
            return true;
        }
    }
    false
}

fn eval_storage_variable(name: &str, session: &DebugSession) -> String {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    let mut slot_info: Option<(U256, &str, &str, &str)> = None;
    for (contract_name, layout) in &session.storage_layouts {
        for entry in &layout.storage {
            if entry.label == name {
                let slot: U256 = entry.slot.parse().unwrap_or_default();
                let type_label = layout
                    .types
                    .get(&entry.type_key)
                    .map(|t| t.label.as_str())
                    .unwrap_or("unknown");
                let encoding = layout
                    .types
                    .get(&entry.type_key)
                    .and_then(|t| t.encoding.as_deref())
                    .unwrap_or("");
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

    let target_address = session
        .identified_contracts
        .iter()
        .find(|(_, v)| v.as_str() == owner_name)
        .map(|(addr, _)| *addr)
        .or_else(|| session.current_address().cloned());

    let mut storage: HashMap<U256, U256> = HashMap::new();
    for (ni, node) in session.debug_arena.iter().enumerate() {
        if target_address.as_ref() != Some(&node.address) {
            continue;
        }
        let max_step = if ni <= session.current_node {
            node.steps.len()
        } else {
            continue;
        };
        for si in 0..max_step {
            let s = &node.steps[si];
            if s.op.get() == 0x55
                && let Some(stack) = &s.stack
                && stack.len() >= 2
            {
                storage.insert(stack[stack.len() - 1], stack[stack.len() - 2]);
            }
        }
    }

    let value = storage.get(&slot).copied().unwrap_or_default();
    let formatted = format_value_by_type(value, type_label);
    format!("{name} ({type_label}) = {formatted}")
}

fn resolve_storage_value(name: &str, session: &DebugSession) -> Option<alloy_primitives::U256> {
    use alloy_primitives::U256;
    use std::collections::HashMap;

    let mut slot_info: Option<(U256, &str, &str)> = None;
    for (contract_name, layout) in &session.storage_layouts {
        for entry in &layout.storage {
            if entry.label == name {
                let slot: U256 = entry.slot.parse().unwrap_or_default();
                let encoding = layout
                    .types
                    .get(&entry.type_key)
                    .and_then(|t| t.encoding.as_deref())
                    .unwrap_or("");
                slot_info = Some((slot, encoding, contract_name.as_str()));
                break;
            }
        }
    }

    let (slot, encoding, owner_name) = slot_info?;
    if encoding == "mapping" {
        return None;
    }

    let target_address = session
        .identified_contracts
        .iter()
        .find(|(_, v)| v.as_str() == owner_name)
        .map(|(addr, _)| *addr)
        .or_else(|| session.current_address().cloned());

    let mut storage: HashMap<U256, U256> = HashMap::new();
    for (ni, node) in session.debug_arena.iter().enumerate() {
        if target_address.as_ref() != Some(&node.address) {
            continue;
        }
        let max_step = if ni <= session.current_node {
            node.steps.len()
        } else {
            continue;
        };
        for si in 0..max_step {
            let s = &node.steps[si];
            if s.op.get() == 0x55
                && let Some(stack) = &s.stack
                && stack.len() >= 2
            {
                storage.insert(stack[stack.len() - 1], stack[stack.len() - 2]);
            }
        }
    }

    Some(storage.get(&slot).copied().unwrap_or_default())
}

// ============ Formatting ============

/// Format a U256 result showing both decimal and hex.
pub fn format_u256_result(val: alloy_primitives::U256) -> String {
    if val.is_zero() {
        return "0 (0x0)".to_string();
    }
    if val.bit_len() <= 64 {
        let dec: u64 = val.to::<u64>();
        format!("{dec} (0x{val:x})")
    } else {
        format!("0x{val:x} ({val})")
    }
}

fn format_value_by_type(value: alloy_primitives::U256, type_label: &str) -> String {
    if type_label == "string" || type_label.starts_with("bytes") {
        variables::decode_short_string(&value)
    } else if type_label.starts_with("uint") {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_literal_conversion() {
        assert_eq!(eval_hex_literal("0xff"), "255 (0xff)");
        assert_eq!(eval_hex_literal("0x0"), "0 (0x0)");
        assert_eq!(eval_hex_literal("0x1"), "1 (0x1)");
        assert_eq!(eval_hex_literal("0x100"), "256 (0x100)");
        assert_eq!(eval_hex_literal("0xdeadbeef"), "3735928559 (0xdeadbeef)");
    }

    #[test]
    fn hex_literal_large_value() {
        let result = eval_hex_literal("0xffffffffffffffffffffffffffffffff");
        assert!(result.starts_with("0x"));
        assert!(result.contains("ffffffffffffffffffffffffffffffff"));
    }

    #[test]
    fn hex_literal_invalid() {
        let result = eval_hex_literal("0xZZZ");
        assert!(result.contains("invalid hex literal"));
    }

    #[test]
    fn format_result_zero() {
        use alloy_primitives::U256;
        assert_eq!(format_u256_result(U256::ZERO), "0 (0x0)");
    }

    #[test]
    fn format_result_small() {
        use alloy_primitives::U256;
        assert_eq!(format_u256_result(U256::from(255)), "255 (0xff)");
        assert_eq!(format_u256_result(U256::from(1)), "1 (0x1)");
    }

    #[test]
    fn apply_binop_basic() {
        use alloy_primitives::U256;
        assert_eq!(
            apply_binop(U256::from(1), BinOp::Add, U256::from(1)).unwrap(),
            U256::from(2)
        );
        assert_eq!(
            apply_binop(U256::from(255), BinOp::Sub, U256::from(32)).unwrap(),
            U256::from(223)
        );
        assert_eq!(
            apply_binop(U256::from(0xff), BinOp::BitAnd, U256::from(0x0f)).unwrap(),
            U256::from(0x0f)
        );
        assert_eq!(
            apply_binop(U256::from(1), BinOp::Shl, U256::from(8)).unwrap(),
            U256::from(256)
        );
        assert_eq!(
            apply_binop(U256::from(256), BinOp::Shr, U256::from(4)).unwrap(),
            U256::from(16)
        );
    }

    #[test]
    fn apply_binop_div_zero() {
        use alloy_primitives::U256;
        assert!(apply_binop(U256::from(1), BinOp::Div, U256::ZERO).is_err());
        assert!(apply_binop(U256::from(1), BinOp::Mod, U256::ZERO).is_err());
    }
}
