// src/main.rs
//
// AI INFO BLOCK PLEASE READ
//
// File Tree:
// src/
// ├── main.rs
// ├── server/
// │   ├── mod.rs
// │   ├── connection.rs
// │   └── routes.rs
// ├── state/
// │   ├── mod.rs
// │   └── app_state.rs
// ├── messages/
// │   └── mod.rs
// ├── bitmap/
// │   └── mod.rs
// ├── ssl/
// │   └── mod.rs
// └── config/
//     └── mod.rs
//
// Files will always start with "// src/<path>/<file>.rs"
// so that you can distinguish between files with the same name
//
// END AI INFO BLOCK

use tokio::signal::unix::{signal, SignalKind};
use tokio::time::{timeout, Duration};
use log::{info, error, LevelFilter};
use env_logger::Builder;
use std::error::Error;
use std::env;

mod server;
mod state;
mod messages;
mod ssl;
mod bitmap;
mod config;

use crate::state::AppState;
use crate::server::run_server;
use crate::ssl::{load_ssl_config, create_tls_acceptor};
use crate::config::REDIS_URL;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    std::env::set_var("RUST_LOG", "warp=warn");
    Builder::new()
        .filter(None, LevelFilter::Warn)
        .format(|buf, record| {
            use std::io::Write;
            let message = record.args().to_string();
            if !message.contains("listening with custom incoming") {
                writeln!(buf, "{}", message)
            } else {
                Ok(())
            }
        })
        .init();

    async_main().await
}

async fn async_main() -> Result<(), Box<dyn Error>> {
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| REDIS_URL.to_string());
    let app_state = AppState::new(&redis_url).await?;  // This now returns Arc<AppState>

    let ssl_config = load_ssl_config().await?;
    let tls_acceptor = create_tls_acceptor(ssl_config);

    info!("Server starting...");

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    let server_handle = tokio::spawn(run_server(app_state, tls_acceptor, shutdown_rx));

    // Create a longer-lived Signal instance
    let mut term_signal = signal(SignalKind::terminate())
        .expect("failed to install SIGTERM handler");

    // Wait for a SIGINT or SIGTERM signal
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received Ctrl+C, initiating graceful shutdown...");
        }
        _ = term_signal.recv() => {
            info!("Received SIGTERM, initiating graceful shutdown...");
        }
    }

    // Send shutdown signal
    let _ = shutdown_tx.send(());

    // Wait for the server to shut down with a timeout
    let shutdown_result = timeout(Duration::from_secs(10), server_handle).await;

    match shutdown_result {
        Ok(Ok(result)) => {
            if let Err(e) = result {
                error!("Server error: {:?}", e);
            }
        }
        Ok(Err(e)) => {
            error!("Error joining server task: {:?}", e);
        }
        Err(_) => {
            error!("Server did not shut down within the allotted time.");
        }
    }

    info!("Server has shut down gracefully.");
    Ok(())
}

