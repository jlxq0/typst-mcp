//! Entry point. Three modes:
//!
//! * `--compile-worker` — read one job from stdin, write one result to stdout, exit.
//!   Spawned by the server; not meant to be run by hand.
//! * `render <template> [out-dir]` — render a shipped template from its fixture, for
//!   checking a template change without a server.
//! * default — serve.

use std::process::ExitCode;

use typst_mcp::config::Config;
use typst_mcp::server::{Server, init_tracing};
use typst_mcp::spawn::WORKER_FLAG;
use typst_mcp::worker;

fn main() -> ExitCode {
    // Checked before anything else and before any runtime starts: a worker is a plain
    // synchronous process that does exactly one thing.
    if std::env::args().any(|a| a == WORKER_FLAG) {
        return ExitCode::from(worker::run() as u8);
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("could not start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(serve()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Configuration errors arrive here, before a port is bound. Printing to
            // stderr as well as tracing means the message survives a logging setup
            // that has not been initialised yet.
            eprintln!("typst-mcp: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    let _telemetry = init_tracing()?;
    let config = Config::from_env()?;
    Server::build(config)?.serve().await
}
