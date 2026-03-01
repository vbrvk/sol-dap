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
                let is_disconnect_restart = matches!(
                    &req.command,
                    Command::Disconnect(args) if args.restart == Some(true)
                );
                let is_disconnect_quit = matches!(&req.command, Command::Disconnect(_))
                    && !is_disconnect_restart;

                let response = handler::handle_request(&req, &mut server, &mut session);
                if let Err(e) = server.respond(response) {
                    tracing::error!("failed to send response: {e}");
                }
                if is_initialize {
                    if let Err(e) = server.send_event(Event::Initialized) {
                        tracing::error!("failed to emit initialized event: {e:?}");
                    }
                }
                if is_disconnect_restart {
                    // Rerun: clear session, stay alive for new initialize+launch
                    tracing::info!("disconnect(restart=true), clearing session");
                    session = None;
                }
                if is_disconnect_quit {
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
