// src/server/connection.rs
use crate::state::AppState;
use crate::messages::Message as AppMessage;
use futures::{StreamExt, SinkExt};
use futures::stream::SplitSink;
use log::{error, info};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, Duration, Instant};
use warp::ws::{WebSocket, Message};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::BROADCAST_CHANNEL_SIZE;
use crate::bitmap::{LockFreeConcurrentBitmap, BitmapMessage};

// Add this at the top of the file, outside of any function
static NEXT_CLIENT_ID: AtomicUsize = AtomicUsize::new(0);

pub struct ClientInfo {
    pub last_pong: Instant,
}

pub type Clients = Arc<RwLock<HashMap<usize, ClientInfo>>>;

pub async fn handle_connection(ws: WebSocket, state: Arc<AppState>, clients: Clients) {
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (client_tx, mut client_rx) = mpsc::channel::<AppMessage>(BROADCAST_CHANNEL_SIZE);
    let mut broadcast_rx = state.subscribe_to_broadcast();

    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::SeqCst);
    info!("New client connected: {}", client_id);

    {
        let mut clients = clients.write().await;
        clients.insert(client_id, ClientInfo {
            last_pong: Instant::now(),
        });
    }

    let client_tx_clone = client_tx.clone();
    state.add_client(client_id, client_tx_clone).await;

    // Create a ping interval
    let mut ping_interval = interval(Duration::from_secs(30)); // Send a ping every 30 seconds

    // Create a timeout check interval
    let mut timeout_check_interval = interval(Duration::from_secs(60)); // Check every minute

    let mut connection_alive = true;
    while connection_alive {
        tokio::select! {
            Some(result) = ws_rx.next() => {
                match result {
                    Ok(msg) => {
                        if msg.is_pong() {
                            let mut clients = clients.write().await;
                            if let Some(client) = clients.get_mut(&client_id) {
                                client.last_pong = Instant::now();
                            }
                        } else if let Err(e) = handle_ws_message(msg, &state, &client_tx, client_id).await {
                            if e.to_string() == "Client requested disconnect" {
                                connection_alive = false;
                            } else {
                                error!("Error handling WebSocket message: {:?}", e);
                                connection_alive = false;
                            }
                        }
                    }
                    Err(e) => {
                        error!("WebSocket error: {:?}", e);
                        connection_alive = false;
                    }
                }
            }
            Ok(broadcast_msg) = broadcast_rx.recv() => {
                if let Err(e) = send_message(&mut ws_tx, &broadcast_msg).await {
                    error!("Error sending broadcast message: {:?}", e);
                    connection_alive = false;
                }
            }
            Some(client_msg) = client_rx.recv() => {
                if let Err(e) = send_message(&mut ws_tx, &client_msg).await {
                    error!("Error sending client message: {:?}", e);
                    connection_alive = false;
                }
            }
            _ = ping_interval.tick() => {
                // Send a ping frame
                if let Err(e) = ws_tx.send(Message::ping(Vec::new())).await {
                    error!("Error sending ping: {:?}", e);
                    connection_alive = false;
                }
            }
            _ = timeout_check_interval.tick() => {
                let mut clients_to_remove = Vec::new();
                let now = Instant::now();
                let timeout_duration = Duration::from_secs(90); // Disconnect after 90 seconds of no pong

                {
                    let clients_read = clients.read().await;
                    for (&id, client) in clients_read.iter() {
                        if now.duration_since(client.last_pong) > timeout_duration {
                            clients_to_remove.push(id);
                        }
                    }
                }

                for id in clients_to_remove {
                    // Disconnect logic here
                    state.remove_client(id).await;
                    clients.write().await.remove(&id);
                    if id == client_id {
                        connection_alive = false;
                        break;
                    }
                }
            }
        }
    }

    // Clean up
    state.remove_client(client_id).await;
    clients.write().await.remove(&client_id);
    info!("Client disconnected: {}", client_id);
}

async fn send_message(ws_tx: &mut SplitSink<WebSocket, Message>, msg: &AppMessage) -> Result<(), Box<dyn std::error::Error>> {
    match msg {
        AppMessage::Update(bitmap_msg) => {
            let encoded = bitmap_msg.encode();
            ws_tx.send(Message::binary(encoded)).await?;
        }
        _ => {
            let msg = serde_json::to_string(msg)?;
            ws_tx.send(Message::text(msg)).await?;
        }
    }
    Ok(())
}

async fn handle_ws_message(
    msg: Message,
    state: &Arc<AppState>,
    client_tx: &mpsc::Sender<AppMessage>,
    client_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if msg.is_binary() {
        let bytes = msg.as_bytes();
        if !bytes.is_empty() {
            let bitmap_msg = BitmapMessage::decode(bytes)?;
            match bitmap_msg {
                BitmapMessage::Update(_, _) | BitmapMessage::BatchUpdate(_) => {
                    handle_bitmap_update(state, bitmap_msg, client_id).await?;
                },
                BitmapMessage::Initialize => {
                    handle_initialize(state, client_tx).await?;
                },
                BitmapMessage::Disconnect => {
                    // Handle disconnect message
                    return Err("Client requested disconnect".into());
                },
                _ => {
                    error!("Unexpected binary message type received by server");
                }
            }
        }
    } else {
        error!("Received non-binary message, which is not supported");
    }
    Ok(())
}

async fn handle_bitmap_update(
    state: &Arc<AppState>,
    bitmap_msg: BitmapMessage,
    client_id: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    match &bitmap_msg {
        BitmapMessage::Update(index, value) => {
            state.update_bitmap(*index as usize, *value)?;
        }
        BitmapMessage::BatchUpdate(updates) => {
            state.apply_bitmap_updates(updates)?;
        }
        BitmapMessage::InitChunk(_, _, _) => {
            // We don't need to update the server's state for InitChunk,
            // but we do need to broadcast it to the client
        }
        BitmapMessage::Initialize => {
            // Initialize should not be handled here
            return Ok(());
        }
        BitmapMessage::Disconnect => {
            // Disconnect should not be handled here
            return Ok(());
        }
    }

    // Use the new batched broadcast method
    state.broadcast(AppMessage::Update(bitmap_msg), Some(client_id)).await;

    Ok(())
}

async fn handle_initialize(
    state: &Arc<AppState>,
    client_tx: &mpsc::Sender<AppMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    let bitmap = state.get_state();
    let total_bits = bitmap.total_size;
    let chunk_size = 100000; // Increased chunk size for efficiency
    let total_chunks = (total_bits + chunk_size - 1) / chunk_size;

    for chunk_index in 0..total_chunks {
        let start = chunk_index * chunk_size;
        let end = (start + chunk_size).min(total_bits);

        let chunk = encode_bitmap_chunk(&bitmap, start, end);
        let init_message = AppMessage::Update(BitmapMessage::InitChunk(chunk_index as u32, total_chunks as u32, chunk));
        client_tx.send(init_message).await?;
    }

    Ok(())
}

fn encode_bitmap_chunk(bitmap: &LockFreeConcurrentBitmap, start: usize, end: usize) -> Vec<u8> {
    let mut chunk = Vec::new();
    let mut current_byte = 0u8;
    let mut bit_count = 0;

    for i in start..end {
        if bitmap.get(i).unwrap_or(false) {
            current_byte |= 1 << (7 - bit_count);
        }

        bit_count += 1;

        if bit_count == 8 {
            chunk.push(current_byte);
            current_byte = 0;
            bit_count = 0;
        }
    }

    // Push the last byte if there are remaining bits
    if bit_count > 0 {
        chunk.push(current_byte);
    }

    chunk
}
