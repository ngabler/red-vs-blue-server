// src/bitmap/tests.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitmap_operations() {
        let bitmap = LockFreeConcurrentBitmap::new(1000);

        // Test set and get
        bitmap.set(42, true).unwrap();
        assert!(bitmap.get(42).unwrap());
        assert!(!bitmap.get(43).unwrap());

        // Test out of bounds
        assert!(bitmap.set(1000, true).is_err());
        assert!(bitmap.get(1000).is_err());

        // Test to_vec
        let vec = bitmap.to_vec();
        assert_eq!(vec.len(), 125); // 1000 bits = 125 bytes
        assert_eq!(vec[5], 0b00010000); // Bit 42 is set
    }

    #[test]
    fn test_bitmap_message_encoding() {
        // Test single update encoding/decoding
        let update = BitmapMessage::Update(12345, true);
        let encoded = update.encode();
        assert_eq!(encoded.len(), 6);
        let decoded = BitmapMessage::decode(&encoded).unwrap();
        assert!(matches!(decoded, BitmapMessage::Update(12345, true)));

        // Test batch update encoding/decoding
        let batch = BitmapMessage::BatchUpdate(vec![(1, true), (2, false), (3, true)]);
        let encoded = batch.encode();
        assert_eq!(encoded.len(), 1 + 3 * 5);
        let decoded = BitmapMessage::decode(&encoded).unwrap();
        if let BitmapMessage::BatchUpdate(updates) = decoded {
            assert_eq!(updates, vec![(1, true), (2, false), (3, true)]);
        } else {
            panic!("Decoded message is not a BatchUpdate");
        }
    }

    #[test]
    fn test_apply_updates() {
        let bitmap = LockFreeConcurrentBitmap::new(1000);
        let updates = vec![(1, true), (2, false), (3, true)];
        bitmap.apply_updates(&updates).unwrap();

        assert!(bitmap.get(1).unwrap());
        assert!(!bitmap.get(2).unwrap());
        assert!(bitmap.get(3).unwrap());
    }
}
