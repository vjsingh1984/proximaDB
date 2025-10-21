//! Collection path codec for WAL directory naming
//!
//! Produces a short, URL-safe slug for collection IDs to avoid long or odd filenames.
//! Uses xxHash64 over the collection_id string and encodes lower 42 bits in base62
//! to produce a 7-character slug (62^7 > 2^42).

pub fn slug_for(collection_id: &str) -> String {
    use crate::utils::hash::HashBuilder;
    use std::hash::Hasher;
    let mut hasher = HashBuilder::with_seed(0).build_xxhash();
    hasher.write(collection_id.as_bytes());
    let h = hasher.finish();
    base62_7(h)
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
