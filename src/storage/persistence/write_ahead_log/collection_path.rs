//! Collection path codec for WAL directory naming
//!
//! Produces a short, URL-safe slug for collection IDs to avoid long or odd filenames.
//! Uses xxHash64 over the collection_id string and encodes lower 42 bits in base62
//! to produce a 7-character slug (62^7 > 2^42).

/// Generate a URL-safe slug for a collection ID
///
/// This function creates a short, 7-character slug from a collection ID using
/// xxHash64 and base62 encoding. The slug is suitable for use in file paths
/// and URLs.
///
/// # Arguments
///
/// * `collection_id` - The collection ID to encode
///
/// # Returns
///
/// A 7-character base62-encoded string
///
/// # Example
///
/// ```ignore
/// let slug = slug_for("my_collection_123");
/// // Returns something like "aB3xY9z"
/// ```
pub fn slug_for(collection_id: &str) -> String {
    use proximadb_kernel::hash::HashBuilder;
    use std::hash::Hasher;
    let mut hasher = HashBuilder::with_seed(0).build_xxhash();
    hasher.write(collection_id.as_bytes());
    let h = hasher.finish();
    base62_7(h)
}

/// Convert a 64-bit value to a 7-character base62 string
///
/// This is an internal helper function that encodes the lower 42 bits of
/// a 64-bit value into a 7-character base62 string.
///
/// # Arguments
///
/// * `v` - The 64-bit value to encode (only lower 42 bits are used)
///
/// # Returns
///
/// A 7-character base62-encoded string
fn base62_7(mut v: u64) -> String {
    const CH: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut buf = [b'0'; 7];
    for i in (0..7).rev() {
        buf[i] = CH[(v % 62) as usize];
        v /= 62;
    }
    String::from_utf8_lossy(&buf).to_string()
}
