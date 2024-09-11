// src/server/mod.rs
use std::sync::Arc;
use std::convert::Infallible;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;
use warp::Filter;
use log::{info, warn};
use std::time::Duration;
use std::net::SocketAddr;

mod connection;
mod routes;

use crate::state::AppState;
use crate::config::{BROADCAST_CHANNEL_SIZE, HTTP_PORT, HTTPS_PORT};

pub use connection::Clients;
pub use routes::create_routes;

pub async fn run_server(
    app_state: Arc<AppState>,
    tls_acceptor: TlsAcceptor,
    mut shutdown_rx: oneshot::Receiver<()>
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let clients: Clients = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

    app_state.initialize_state().await?;

    let routes = create_routes(app_state.clone(), clients.clone());

    info!("WebSocket server running on ws://0.0.0.0:{}, wss://0.0.0.0:{}", HTTP_PORT, HTTPS_PORT);
    info!("Health check available at http://0.0.0.0:{}/health-check, https://0.0.0.0:{}/health-check", HTTP_PORT, HTTPS_PORT);
    info!("Index page available at http://0.0.0.0:{}/", HTTP_PORT);

    let (http_handle, https_handle) = run_http_and_https_server(routes, tls_acceptor).await;

    // Wait for shutdown signal
    tokio::select! {
        _ = &mut shutdown_rx => {
            info!("Shutdown signal received, stopping server...");
        }
        _ = http_handle => {
            warn!("HTTP server has stopped unexpectedly");
        }
        _ = https_handle => {
            warn!("HTTPS server has stopped unexpectedly");
        }
    }

    // Perform cleanup
    // Close all WebSocket connections
    {
        let mut clients_lock = clients.write().await;
        clients_lock.clear();
    }

    // Wait for ongoing operations to complete
    tokio::time::sleep(Duration::from_secs(1)).await;

    info!("Server shutdown complete");
    Ok(())
}

async fn run_http_and_https_server(
    routes: impl Filter<Extract = impl warp::Reply, Error = Infallible> + Clone + Send + Sync + 'static,
    tls_acceptor: TlsAcceptor
) -> (tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>) {
    let http_addr: SocketAddr = ([0, 0, 0, 0], HTTP_PORT).into();
    let https_addr: SocketAddr = ([0, 0, 0, 0], HTTPS_PORT).into();

    let http_listener = tokio::net::TcpListener::bind(http_addr).await.expect("Failed to bind to HTTP port");
    let https_listener = tokio::net::TcpListener::bind(https_addr).await.expect("Failed to bind to HTTPS port");

    let routes_http = routes.clone();
    let http_handle = tokio::spawn(async move {
        loop {
            match http_listener.accept().await {
                Ok((stream, _)) => {
                    let routes = routes_http.clone();
                    tokio::spawn(async move {
                        warp::serve(routes)
                            .run_incoming(futures::stream::once(async { Ok::<_, std::io::Error>(stream) }))
                            .await;
                    });
                }
                Err(e) => log::error!("Failed to accept HTTP connection: {:?}", e),
            }
        }
    });

    let routes_https = routes;
    let https_handle = tokio::spawn(async move {
        loop {
            match https_listener.accept().await {
                Ok((stream, _)) => {
                    let tls_acceptor = tls_acceptor.clone();
                    let routes = routes_https.clone();
                    tokio::spawn(async move {
                        match tls_acceptor.accept(stream).await {
                            Ok(tls_stream) => {
                                warp::serve(routes)
                                    .run_incoming(futures::stream::once(async { Ok::<_, std::io::Error>(tls_stream) }))
                                    .await;
                            }
                            Err(e) => log::error!("TLS handshake failed: {:?}", e),
                        }
                    });
                }
                Err(e) => log::error!("Failed to accept HTTPS connection: {:?}", e),
            }
        }
    });

    (http_handle, https_handle)
}
