//! Collection path codec for WAL directory naming.
//!
//! Produces a short, URL-safe slug for collection IDs to avoid long or odd filenames.
//! Uses xxHash64 over the collection_id string and encodes lower 42 bits in base62
//! to produce a 7-character slug (62^7 > 2^42).

/// Generate a URL-safe 7-character slug for a collection ID.
pub fn slug_for(collection_id: &str) -> String {
    use proximadb_kernel::hash::HashBuilder;
    use std::hash::Hasher;
    let mut hasher = HashBuilder::with_seed(0).build_xxhash();
    hasher.write(collection_id.as_bytes());
    base62_7(hasher.finish())
}

fn base62_7(mut v: u64) -> String {
    const CH: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut buf = [b'0'; 7];
    for i in (0..7).rev() {
        buf[i] = CH[(v % 62) as usize];
        v /= 62;
    }
    String::from_utf8_lossy(&buf).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_for_is_stable_url_safe_and_fixed_width() {
        let first = slug_for("tenant-a/collection-01");
        let second = slug_for("tenant-a/collection-01");

        assert_eq!(first, second);
        assert_eq!(first.len(), 7);
        assert!(first.bytes().all(|b| b.is_ascii_alphanumeric()));
    }

    #[test]
    fn slug_for_distinguishes_different_collection_ids() {
        assert_ne!(
            slug_for("tenant-a/collection-01"),
            slug_for("tenant-a/collection-02")
        );
    }

    #[test]
    fn base62_encoder_pads_and_uses_expected_alphabet_boundaries() {
        assert_eq!(base62_7(0), "0000000");
        assert_eq!(base62_7(9), "0000009");
        assert_eq!(base62_7(10), "000000A");
        assert_eq!(base62_7(35), "000000Z");
        assert_eq!(base62_7(36), "000000a");
        assert_eq!(base62_7(61), "000000z");
        assert_eq!(base62_7(62), "0000010");
    }
}
