use std::io::{Read, Write};

use dap::prelude::*;

use crate::config::LaunchConfig;
use crate::session::DebugSession;

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
                supports_step_back: Some(false),
                supports_terminate_request: Some(false),
                ..Default::default()
            };

            if let Err(e) = server.send_event(Event::Initialized) {
                tracing::error!("failed to emit initialized event: {e:?}");
            }

            req.clone().success(ResponseBody::Initialize(capabilities))
        }
        Command::ConfigurationDone => req.clone().success(ResponseBody::ConfigurationDone),
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

                    let _ = server.send_event(Event::Stopped(events::StoppedEventBody {
                        reason: types::StoppedEventReason::Entry,
                        description: None,
                        thread_id: Some(1),
                        preserve_focus_hint: None,
                        text: None,
                        all_threads_stopped: None,
                        hit_breakpoint_ids: None,
                    }));

                    req.clone().success(ResponseBody::Launch)
                }
                Err(e) => req.clone().error(&format!("launch failed: {e:#}")),
            }
        }
        Command::Threads => {
            let body = responses::ThreadsResponse {
                threads: vec![types::Thread {
                    id: 1,
                    name: "main".to_string(),
                }],
            };

            req.clone().success(ResponseBody::Threads(body))
        }
        Command::SetBreakpoints(_) => {
            let body = responses::SetBreakpointsResponse {
                breakpoints: Vec::new(),
            };

            req.clone().success(ResponseBody::SetBreakpoints(body))
        }
        _ => req.clone().error("command not supported"),
    }
}
