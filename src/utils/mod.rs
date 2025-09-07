// Internal utilities module - replacing external dependencies
// This module provides internal implementations of common utilities
// to reduce external dependencies and improve performance

pub mod uuid;
pub mod hash;
pub mod checksum;
pub mod encoding;
pub mod glob;
pub mod cache;
pub mod bitmap;
pub mod btree;
pub mod skiplist;

// Re-export commonly used items
pub use self::uuid::{Uuid, UuidGenerator};
pub use self::hash::{FastHash, HashBuilder};
pub use self::checksum::{Crc32, Checksum};
pub use self::encoding::{base64_encode, base64_decode};
pub use self::glob::{GlobPattern, GlobMatcher};
pub use self::cache::{LruCache, CacheEntry};
pub use self::bitmap::{RoaringBitmap, BitmapIteratorAll};
pub use self::btree::{BPlusTree, BTreeIterator};
pub use self::skiplist::{SkipList, SkipListIterator};
