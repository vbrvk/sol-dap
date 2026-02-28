use std::io::{BufReader, BufWriter};

use dap::prelude::*;

mod config;
mod handler;
mod launch;
mod session;

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
                let response = handler::handle_request(&req, &mut server, &mut session);
                if let Err(e) = server.respond(response) {
                    tracing::error!("failed to send response: {e}");
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
