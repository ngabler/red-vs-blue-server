// src/server/routes.rs
use std::convert::Infallible;
use warp::{Filter, Reply};
use crate::state::AppState;
use std::sync::Arc;
use super::connection::{handle_connection, Clients};
use log::error;

use crate::config::{WS_HOST, WSS_HOST};

pub fn create_routes(
    state: Arc<AppState>,
    clients: Clients,
) -> impl Filter<Extract = impl Reply, Error = Infallible> + Clone {
    let state = warp::any().map(move || state.clone());
    let clients_filter = warp::any().map(move || clients.clone());

    let ws_route = warp::path("ws")
        .and(warp::ws())
        .and(state.clone())
        .and(clients_filter.clone())
        .map(|ws: warp::ws::Ws, state: Arc<AppState>, clients: Clients| {
            ws.max_write_buffer_size(1024 * 1024)
               .max_message_size(1024 * 1024)
               .max_frame_size(1024 * 1024)
               .on_upgrade(move |socket| handle_connection(socket, state, clients))
        });

    let health_check = warp::path("health-check")
        .map(|| warp::reply::with_status("OK", warp::http::StatusCode::OK))
        .with(warp::log::custom(|_| {}));  // Prevent logging for health-check

    let index = warp::path::end()
        .map(move || warp::reply::html(format!("<html><body>{}</body></html>", WSS_HOST)));

    // Redirect ws.gabler.app to wss.gabler.app
    let ws_to_wss_redirect = warp::host::exact(WS_HOST)
        .map(|| {
            let uri = warp::http::Uri::builder()
                .scheme("https")
                .authority(WSS_HOST)
                .path_and_query("/")
                .build()
                .expect("Failed to build URI");
            warp::redirect::permanent(uri)
        });

    // Main routes for wss.gabler.app
    let wss_routes = warp::host::exact(WSS_HOST)
        .and(
            ws_route
                .or(health_check)
                .or(index)
        );

    ws_to_wss_redirect
        .or(wss_routes)
        .with(warp::cors().allow_any_origin())
        .recover(handle_rejection)
}

async fn handle_rejection(err: warp::Rejection) -> Result<impl Reply, Infallible> {
    let (code, message) = if err.is_not_found() {
        (warp::http::StatusCode::NOT_FOUND, "Not Found")
    } else if err.find::<warp::reject::PayloadTooLarge>().is_some() {
        (warp::http::StatusCode::BAD_REQUEST, "Payload too large")
    } else {
        error!("unhandled error: {:?}", err);
        (warp::http::StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error")
    };

    Ok(warp::reply::with_status(message.to_string(), code))
}
