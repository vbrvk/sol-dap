use alloy_primitives::hex;

use std::io::{Read, Write};
use std::path::PathBuf;

use dap::prelude::*;

use crate::config::LaunchConfig;
use crate::session::{DebugSession, StopReason};
use crate::{source_map, variables};

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

fn emit_stopped<R: Read, W: Write>(
    server: &mut dap::server::Server<R, W>,
    session: &Option<DebugSession>,
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
                let step_idx = if i == session.current_node {
                    session.current_step
                } else {
                    0
                };
                let Some(step) = node.steps.get(step_idx) else {
                    continue;
                };

                let contract_name = session
                    .identified_contracts
                    .get(&node.address)
                    .map(|s| s.as_str())
                    .unwrap_or("Unknown");

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
                (3, Some(node), _) => variables::calldata_variables(node),
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
                            contract_name,
                            layout,
                        )
                    } else {
                        Vec::new()
                    }
                }
                (7, Some(node), _) => {
                    let contract_name = session
                        .identified_contracts
                        .get(&node.address)
                        .map(|s| s.as_str())
                        .unwrap_or("Unknown");
                    let fn_name = if node.calldata.len() >= 4 {
                        let sel = format!("0x{}", alloy_primitives::hex::encode(&node.calldata[..4]));
                        session.method_identifiers.get(&sel).map(|s| s.as_str())
                    } else {
                        None
                    };
                    variables::context_variables(
                        node,
                        frame_idx,
                        &session.debug_arena,
                        contract_name,
                        fn_name,
                        &session.method_identifiers,
                    )
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

            let result = match args.expression.as_str() {
                "pc" => step.pc.to_string(),
                "op" => step.op.to_string(),
                "gas" => step.gas_remaining.to_string(),
                "address" => session
                    .current_address()
                    .map(|a| a.to_string())
                    .unwrap_or_default(),
                "memory.length" => step
                    .memory
                    .as_ref()
                    .map(|m| m.len().to_string())
                    .unwrap_or_else(|| "0".to_string()),
                "calldata" => session
                    .current_debug_node()
                    .map(|n| format!("0x{}", hex::encode(&n.calldata)))
                    .unwrap_or_default(),
                "returndata" => format!("0x{}", hex::encode(&step.returndata)),
                "depth" => session.current_node.to_string(),
                "node" => session.current_node.to_string(),
                "step" => session.current_step.to_string(),

                s if s.starts_with("stack[") && s.ends_with(']') => {
                    let idx_str = &s[6..s.len() - 1];
                    match idx_str.parse::<usize>() {
                        Ok(idx) => {
                            if let Some(stack) = &step.stack {
                                if idx < stack.len() {
                                    stack[stack.len() - 1 - idx].to_string()
                                } else {
                                    "stack index out of bounds".to_string()
                                }
                            } else {
                                "stack not available".to_string()
                            }
                        }
                        Err(_) => "invalid stack index".to_string(),
                    }
                }
                _ => "not implemented".to_string(),
            };

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
            let config = match session.as_ref() {
                Some(s) => s.launch_config.clone(),
                None => return req.clone().error("no active debug session"),
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
                use alloy_primitives::hex;
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
