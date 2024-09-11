// src/config/mod.rs
pub const BROADCAST_CHANNEL_SIZE: usize = 100_000;
pub const REDIS_URL: &str = "redis://127.0.0.1:6379";
pub const REDIS_SYNC_INTERVAL: u64 = 1;
pub const HTTP_PORT: u16 = 80;
pub const HTTPS_PORT: u16 = 443;
pub const WS_HOST: &str = "ws.gabler.app";
pub const WSS_HOST: &str = "wss.gabler.app";
pub const BITMAP_TOTAL_SIZE: usize = 1_000_000;
pub const BATCH_INTERVAL_MICROS: u64 = 7812; // 7.8125 ms in microseconds
