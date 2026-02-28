use std::io::{BufReader, BufWriter};

use dap::prelude::*;

use sol_dap::{handler, session};

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("sol-dap starting...");

    let input = BufReader::new(std::io::stdin());
    let output = BufWriter::new(std::io::stdout());
    let mut server = Server::new(input, output);
    let mut session: Option<session::DebugSession> = None;

    loop {
        match server.poll_request() {
            Ok(Some(req)) => {
                let is_initialize = matches!(&req.command, Command::Initialize(_));
                let is_disconnect = matches!(&req.command, Command::Disconnect(_));
                let response = handler::handle_request(&req, &mut server, &mut session);
                if let Err(e) = server.respond(response) {
                    tracing::error!("failed to send response: {e}");
                }
                // DAP spec: Initialized event must be sent AFTER the initialize response.
                if is_initialize {
                    if let Err(e) = server.send_event(Event::Initialized) {
                        tracing::error!("failed to emit initialized event: {e:?}");
                    }
                }
                // Exit after disconnect so Zed can restart with a fresh process.
                if is_disconnect {
                    tracing::info!("disconnect received, exiting");
                    break;
                }
            }
            Ok(None) => {
                tracing::info!("client disconnected (EOF)");
                break;
            }
            Err(e) => {
                tracing::error!("error reading request: {e:?}");
                break;
            }
        }
    }
}
