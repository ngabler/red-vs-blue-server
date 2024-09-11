// src/bitmap/mod.rs
use std::sync::atomic::{AtomicU64, Ordering};
use serde::{Serialize, Deserialize};

#[repr(align(64))]
struct AlignedSegment(AtomicU64);

pub struct LockFreeConcurrentBitmap {
    segments: Vec<AlignedSegment>,
    segment_size: usize,
    pub total_size: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum BitmapMessage {
    Initialize,
    Update(u32, bool),
    BatchUpdate(Vec<(u32, bool)>),
    InitChunk(u32, u32, Vec<u8>),
    Disconnect,
}

impl LockFreeConcurrentBitmap {
    pub fn new(total_size: usize) -> Self {
        let segment_size = 64; // Each AtomicU64 can hold 64 bits
        let num_segments = (total_size + segment_size - 1) / segment_size;
        let segments = (0..num_segments)
            .map(|_| AlignedSegment(AtomicU64::new(0)))
            .collect();

        LockFreeConcurrentBitmap {
            segments,
            segment_size,
            total_size,
        }
    }

    pub fn set(&self, index: usize, value: bool) -> Result<(), &'static str> {
        if index >= self.total_size {
            return Err("Index out of bounds");
        }

        let segment_index = index / self.segment_size;
        let bit_index = index % self.segment_size;
        let mask = 1u64 << bit_index;

        let segment = &self.segments[segment_index].0;

        if value {
            segment.fetch_or(mask, Ordering::Relaxed);
        } else {
            segment.fetch_and(!mask, Ordering::Relaxed);
        }

        Ok(())
    }

    pub fn get(&self, index: usize) -> Result<bool, &'static str> {
        if index >= self.total_size {
            return Err("Index out of bounds");
        }

        let segment_index = index / self.segment_size;
        let bit_index = index % self.segment_size;
        let mask = 1u64 << bit_index;

        let segment = &self.segments[segment_index].0;
        let value = segment.load(Ordering::Relaxed);

        Ok((value & mask) != 0)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let num_bytes = (self.total_size + 7) / 8;
        let mut result = Vec::with_capacity(num_bytes);

        for chunk in self.segments.chunks(2) {
            let lower = chunk[0].0.load(Ordering::Relaxed);
            let upper = chunk.get(1).map_or(0, |aligned_segment| aligned_segment.0.load(Ordering::Relaxed));

            result.extend_from_slice(&lower.to_le_bytes());
            if result.len() < num_bytes {
                result.extend_from_slice(&upper.to_le_bytes());
            }
        }

        result.truncate(num_bytes);
        result
    }

    pub fn apply_updates(&self, updates: &[(u32, bool)]) -> Result<(), &'static str> {
        for &(index, value) in updates {
            self.set(index as usize, value)?;
        }
        Ok(())
    }
}

impl BitmapMessage {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            BitmapMessage::Initialize => vec![255],
            BitmapMessage::Update(index, value) => {
                let mut bytes = Vec::with_capacity(6);
                bytes.push(0);
                bytes.extend_from_slice(&index.to_be_bytes());
                bytes.push(if *value { 1 } else { 0 });
                bytes
            }
            BitmapMessage::BatchUpdate(updates) => {
                let mut bytes = Vec::with_capacity(1 + updates.len() * 5);
                bytes.push(updates.len() as u8);
                for (index, value) in updates {
                    bytes.extend_from_slice(&index.to_be_bytes());
                    bytes.push(if *value { 1 } else { 0 });
                }
                bytes
            }
            BitmapMessage::InitChunk(chunk_index, total_chunks, chunk) => {
                let mut bytes = Vec::with_capacity(9 + chunk.len());
                bytes.push(254);
                bytes.extend_from_slice(&chunk_index.to_be_bytes());
                bytes.extend_from_slice(&total_chunks.to_be_bytes());
                bytes.extend(chunk);
                bytes
            }
            BitmapMessage::Disconnect => vec![253], // New special byte for Disconnect
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.is_empty() {
            return Err("Empty message");
        }

        match bytes[0] {
            255 => Ok(BitmapMessage::Initialize),
            0 => {
                if bytes.len() != 6 {
                    return Err("Invalid single update message length");
                }
                let index = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                let value = bytes[5] != 0;
                Ok(BitmapMessage::Update(index, value))
            }
            254 => {
                if bytes.len() < 9 {
                    return Err("Invalid InitChunk message length");
                }
                let chunk_index = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                let total_chunks = u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
                let chunk = bytes[9..].to_vec();
                Ok(BitmapMessage::InitChunk(chunk_index, total_chunks, chunk))
            }
            253 => Ok(BitmapMessage::Disconnect), // New case for Disconnect
            count => {
                if bytes.len() != 1 + (count as usize) * 5 {
                    return Err("Invalid batch update message length");
                }
                let mut updates = Vec::with_capacity(count as usize);
                for chunk in bytes[1..].chunks_exact(5) {
                    let index = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    let value = chunk[4] != 0;
                    updates.push((index, value));
                }
                Ok(BitmapMessage::BatchUpdate(updates))
            }
        }
    }
}

