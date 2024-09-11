// src/messages/mod.rs
use crate::bitmap::BitmapMessage;
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Message {
    Initialize,
    Update(BitmapMessage),
}
