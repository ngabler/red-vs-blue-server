// src/state/app_state.rs
use redis::{Client, AsyncCommands};
use redis::aio::MultiplexedConnection;
use tokio::sync::{RwLock, mpsc, broadcast, Mutex};
use tokio::time::{Duration, interval};
use std::sync::Arc;
use std::collections::HashMap;
use bitvec::prelude::*;
use log::{error, info};
use crate::bitmap::LockFreeConcurrentBitmap;
use crate::messages::Message as AppMessage;
use crate::config::{BROADCAST_CHANNEL_SIZE, BITMAP_TOTAL_SIZE, REDIS_SYNC_INTERVAL, BATCH_INTERVAL_MICROS};

pub struct AppState {
    redis: Client,
    bitmap: Arc<LockFreeConcurrentBitmap>,
    clients: Arc<RwLock<HashMap<usize, mpsc::Sender<AppMessage>>>>,
    broadcast_tx: broadcast::Sender<AppMessage>,
    broadcast_queue: Arc<Mutex<Vec<(AppMessage, Option<usize>)>>>,
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        AppState {
            redis: self.redis.clone(),
            bitmap: Arc::clone(&self.bitmap),
            clients: Arc::clone(&self.clients),
            broadcast_tx: self.broadcast_tx.clone(),
            broadcast_queue: Arc::clone(&self.broadcast_queue),
        }
    }
}

impl AppState {
    pub async fn new(redis_url: &str) -> Result<Arc<Self>, redis::RedisError> {
        let client = Client::open(redis_url)?;
        let mut conn = client.get_multiplexed_async_connection().await?;

        // Attempt to ping Redis to verify the connection and authentication
        redis::cmd("PING").query_async(&mut conn).await?;

        let (broadcast_tx, _) = broadcast::channel(BROADCAST_CHANNEL_SIZE);
        let app_state = Arc::new(Self {
            redis: client,
            bitmap: Arc::new(LockFreeConcurrentBitmap::new(BITMAP_TOTAL_SIZE)),
            clients: Arc::new(RwLock::new(HashMap::new())),
            broadcast_tx,
            broadcast_queue: Arc::new(Mutex::new(Vec::new())),
        });

        // Initialize state from Redis
        app_state.initialize_state().await?;

        // Start the periodic Redis update task
        let app_state_clone = Arc::clone(&app_state);
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(REDIS_SYNC_INTERVAL));
            loop {
                interval.tick().await;
                if let Err(e) = app_state_clone.update_redis().await {
                    error!("Failed to update Redis: {:?}", e);
                }
            }
        });

        // Start the broadcast batching task
        let state_clone = Arc::clone(&app_state);
        tokio::spawn(Self::batch_broadcast_task(state_clone));

        Ok(app_state)
    }

    async fn get_connection(&self) -> Result<MultiplexedConnection, redis::RedisError> {
        self.redis.get_multiplexed_async_connection().await
    }

    pub async fn initialize_state(&self) -> Result<(), redis::RedisError> {
        let mut conn = self.get_connection().await?;
        let state: Option<Vec<u8>> = conn.get("checkbox_state").await?;
        match state {
            Some(s) if !s.is_empty() => {
                let bitvec = BitVec::<u8, Msb0>::from_vec(s);
                for (index, bit) in bitvec.iter().enumerate() {
                    self.bitmap.set(index, *bit)
                        .map_err(|e| redis::RedisError::from((redis::ErrorKind::ClientError, e)))?;
                }
                info!("Initialized state from Redis. Total bits: {}", bitvec.len());
            },
            _ => {
                info!("Initialized empty state. All checkboxes unchecked.");
            }
        }
        Ok(())
    }

    pub fn get_state(&self) -> Arc<LockFreeConcurrentBitmap> {
        Arc::clone(&self.bitmap)
    }

    pub fn update_bitmap(&self, index: usize, value: bool) -> Result<(), &'static str> {
        self.bitmap.set(index, value)
    }

    pub fn apply_bitmap_updates(&self, updates: &[(u32, bool)]) -> Result<(), &'static str> {
        self.bitmap.apply_updates(updates)
    }

    pub async fn broadcast(&self, message: AppMessage, exclude_client_id: Option<usize>) {
        let mut queue = self.broadcast_queue.lock().await;
        queue.push((message, exclude_client_id));
    }

    async fn batch_broadcast_task(state: Arc<AppState>) {
        let mut interval = interval(Duration::from_micros(BATCH_INTERVAL_MICROS));
        loop {
            interval.tick().await;
            state.flush_broadcast_queue().await;
        }
    }

    async fn flush_broadcast_queue(&self) {
        let messages = {
            let mut queue = self.broadcast_queue.lock().await;
            std::mem::take(&mut *queue)
        };

        if !messages.is_empty() {
            let clients = self.clients.read().await;
            for (message, exclude_id) in messages {
                for (&client_id, sender) in clients.iter() {
                    if Some(client_id) != exclude_id {
                        if let Err(e) = sender.send(message.clone()).await {
                            error!("Failed to send batched message to client {}: {:?}", client_id, e);
                        }
                    }
                }
            }
        }
    }

    pub async fn add_client(&self, id: usize, sender: mpsc::Sender<AppMessage>) {
        self.clients.write().await.insert(id, sender);
    }

    pub async fn remove_client(&self, id: usize) {
        self.clients.write().await.remove(&id);
    }

    pub fn subscribe_to_broadcast(&self) -> broadcast::Receiver<AppMessage> {
        self.broadcast_tx.subscribe()
    }

    async fn update_redis(&self) -> Result<(), redis::RedisError> {
        let mut conn = self.get_connection().await?;
        let bytes: Vec<u8> = self.bitmap.to_vec();
        conn.set("checkbox_state", bytes).await?;
        info!("Updated state in Redis");
        Ok(())
    }
}
