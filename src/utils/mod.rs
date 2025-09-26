// Internal utilities module - replacing external dependencies
// This module provides internal implementations of common utilities
// to reduce external dependencies and improve performance

pub mod bitmap;
pub mod btree;
pub mod cache;
pub mod checksum;
pub mod encoding;
pub mod glob;
pub mod hash;
pub mod skiplist;
pub mod storage_path;
pub mod uuid;

// Re-export commonly used items
pub use self::bitmap::{BitmapIteratorAll, RoaringBitmap};
pub use self::btree::{BPlusTree, BTreeIterator};
pub use self::cache::{CacheEntry, LruCache};
pub use self::checksum::{Checksum, Crc32};
pub use self::encoding::{base64_decode, base64_encode};
pub use self::glob::{GlobMatcher, GlobPattern};
pub use self::hash::{FastHash, HashBuilder};
pub use self::skiplist::{SkipList, SkipListIterator};
pub use self::storage_path::StoragePath;
pub use self::uuid::{Uuid, UuidGenerator};
