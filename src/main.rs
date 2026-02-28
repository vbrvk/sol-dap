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
                tracing::info!("received request: {:?}", std::mem::discriminant(&req.command));
                let response = handler::handle_request(&req, &mut server, &mut session);
                tracing::info!("sending response for seq={}", req.seq);
                if let Err(e) = server.respond(response) {
                    tracing::error!("failed to send response: {e}");
                }
                tracing::info!("response sent for seq={}", req.seq);
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
