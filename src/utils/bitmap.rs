//! Roaring bitmap implementation for ProximaDB
//!
//! This module provides an internal Roaring bitmap implementation to replace
//! external bitmap dependencies. It's optimized for vector database use cases
//! like ID sets, filter operations, and sparse data structures.
//!
//! # Features
//! - Compressed bitmap representation using array and bitmap containers
//! - Efficient set operations (union, intersection, difference, XOR)
//! - Memory-efficient storage for sparse and dense bit patterns
//! - Iterator support for efficient traversal
//! - Serialization/deserialization support
//! - Thread-safe operations
//!
//! # Example
//! ```rust
//! use proximadb::utils::bitmap::RoaringBitmap;
//!
//! let mut bitmap1 = RoaringBitmap::new();
//! bitmap1.insert(1);
//! bitmap1.insert(100);
//! bitmap1.insert(1000);
//!
//! let mut bitmap2 = RoaringBitmap::new();
//! bitmap2.insert(1);
//! bitmap2.insert(200);
//!
//! let intersection = bitmap1.intersect(&bitmap2);
//! assert!(intersection.contains(1));
//! assert!(!intersection.contains(100));
//! ```

use std::collections::BTreeMap;
use std::fmt;

/// Error types for bitmap operations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BitmapError {
    /// Invalid value for bitmap operations
    InvalidValue,
    /// Serialization error
    SerializationError(String),
    /// Container error
    ContainerError(String),
}

impl fmt::Display for BitmapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BitmapError::InvalidValue => write!(f, "Invalid value for bitmap operation"),
            BitmapError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            BitmapError::ContainerError(msg) => write!(f, "Container error: {}", msg),
        }
    }
}

impl std::error::Error for BitmapError {}

/// Container types for different density patterns
#[derive(Debug, Clone)]
enum Container {
    /// Array container for sparse data (< 4096 elements)
    Array(ArrayContainer),
    /// Bitmap container for dense data (>= 4096 elements)
    Bitmap(BitmapContainer),
    /// Run container for consecutive runs (future optimization)
    Run(RunContainer),
}

impl Container {
    /// Insert a value into the container
    fn insert(&mut self, value: u16) -> bool {
        match self {
            Container::Array(array) => array.insert(value),
            Container::Bitmap(bitmap) => bitmap.insert(value),
            Container::Run(run) => run.insert(value),
        }
    }

    /// Remove a value from the container
    fn remove(&mut self, value: u16) -> bool {
        match self {
            Container::Array(array) => array.remove(value),
            Container::Bitmap(bitmap) => bitmap.remove(value),
            Container::Run(run) => run.remove(value),
        }
    }

    /// Check if container contains a value
    fn contains(&self, value: u16) -> bool {
        match self {
            Container::Array(array) => array.contains(value),
            Container::Bitmap(bitmap) => bitmap.contains(value),
            Container::Run(run) => run.contains(value),
        }
    }

    /// Get cardinality (number of elements)
    fn cardinality(&self) -> u32 {
        match self {
            Container::Array(array) => array.cardinality(),
            Container::Bitmap(bitmap) => bitmap.cardinality(),
            Container::Run(run) => run.cardinality(),
        }
    }

    /// Convert to optimal container type based on cardinality
    fn optimize(self) -> Container {
        let cardinality = self.cardinality();

        match self {
            Container::Array(array) => {
                if cardinality >= 4096 {
                    // Convert to bitmap container
                    let mut bitmap = BitmapContainer::new();
                    for value in array.values.iter() {
                        bitmap.insert(*value);
                    }
                    Container::Bitmap(bitmap)
                } else {
                    Container::Array(array)
                }
            }
            Container::Bitmap(bitmap) => {
                if cardinality < 4096 {
                    // Convert to array container
                    let mut array = ArrayContainer::new();
                    for i in 0u32..65536 {
                        if bitmap.contains(i as u16) {
                            array.insert(i as u16);
                        }
                    }
                    Container::Array(array)
                } else {
                    Container::Bitmap(bitmap)
                }
            }
            Container::Run(_) => self, // Keep run containers as-is for now
        }
    }

    /// Union with another container
    fn union(&self, other: &Container) -> Container {
        match (self, other) {
            (Container::Array(a1), Container::Array(a2)) => Container::Array(a1.union(a2)),
            (Container::Bitmap(b1), Container::Bitmap(b2)) => Container::Bitmap(b1.union(b2)),
            (Container::Array(array), Container::Bitmap(bitmap))
            | (Container::Bitmap(bitmap), Container::Array(array)) => {
                let mut result = bitmap.clone();
                for &value in &array.values {
                    result.insert(value);
                }
                Container::Bitmap(result)
            }
            // Simplified handling for run containers
            (a, b) => {
                let mut result = a.clone();
                for i in 0u32..65536 {
                    if b.contains(i as u16) {
                        result.insert(i as u16);
                    }
                }
                result.optimize()
            }
        }
    }

    /// Intersection with another container
    fn intersect(&self, other: &Container) -> Container {
        match (self, other) {
            (Container::Array(a1), Container::Array(a2)) => Container::Array(a1.intersect(a2)),
            (Container::Bitmap(b1), Container::Bitmap(b2)) => Container::Bitmap(b1.intersect(b2)),
            (Container::Array(array), Container::Bitmap(bitmap))
            | (Container::Bitmap(bitmap), Container::Array(array)) => {
                let mut result = ArrayContainer::new();
                for &value in &array.values {
                    if bitmap.contains(value) {
                        result.insert(value);
                    }
                }
                Container::Array(result)
            }
            // Simplified handling for run containers
            (a, b) => {
                let mut result = ArrayContainer::new();
                for i in 0u32..65536 {
                    if a.contains(i as u16) && b.contains(i as u16) {
                        result.insert(i as u16);
                    }
                }
                Container::Array(result)
            }
        }
    }

    /// Get all values in the container
    fn iter(&self) -> Box<dyn Iterator<Item = u16> + '_> {
        match self {
            Container::Array(array) => Box::new(array.values.iter().copied()),
            Container::Bitmap(bitmap) => Box::new(BitmapIterator::new(&bitmap.bits)),
            Container::Run(run) => Box::new(run.iter()),
        }
    }
}

/// Array container for sparse data
#[derive(Debug, Clone)]
struct ArrayContainer {
    values: Vec<u16>,
}

impl ArrayContainer {
    fn new() -> Self {
        ArrayContainer { values: Vec::new() }
    }

    fn insert(&mut self, value: u16) -> bool {
        match self.values.binary_search(&value) {
            Ok(_) => false, // Already exists
            Err(pos) => {
                self.values.insert(pos, value);
                true
            }
        }
    }

    fn remove(&mut self, value: u16) -> bool {
        match self.values.binary_search(&value) {
            Ok(pos) => {
                self.values.remove(pos);
                true
            }
            Err(_) => false,
        }
    }

    fn contains(&self, value: u16) -> bool {
        self.values.binary_search(&value).is_ok()
    }

    fn cardinality(&self) -> u32 {
        self.values.len() as u32
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn union(&self, other: &ArrayContainer) -> ArrayContainer {
        let mut result = Vec::with_capacity(self.values.len() + other.values.len());
        let mut i = 0;
        let mut j = 0;

        while i < self.values.len() && j < other.values.len() {
            if self.values[i] < other.values[j] {
                result.push(self.values[i]);
                i += 1;
            } else if self.values[i] > other.values[j] {
                result.push(other.values[j]);
                j += 1;
            } else {
                result.push(self.values[i]);
                i += 1;
                j += 1;
            }
        }

        // Add remaining elements
        result.extend_from_slice(&self.values[i..]);
        result.extend_from_slice(&other.values[j..]);

        ArrayContainer { values: result }
    }

    fn intersect(&self, other: &ArrayContainer) -> ArrayContainer {
        let mut result = Vec::new();
        let mut i = 0;
        let mut j = 0;

        while i < self.values.len() && j < other.values.len() {
            if self.values[i] < other.values[j] {
                i += 1;
            } else if self.values[i] > other.values[j] {
                j += 1;
            } else {
                result.push(self.values[i]);
                i += 1;
                j += 1;
            }
        }

        ArrayContainer { values: result }
    }
}

/// Bitmap container for dense data
#[derive(Debug, Clone)]
struct BitmapContainer {
    bits: [u64; 1024], // 64K bits = 1024 * 64 bits
}

impl BitmapContainer {
    fn new() -> Self {
        BitmapContainer { bits: [0; 1024] }
    }

    fn insert(&mut self, value: u16) -> bool {
        let word_index = (value as usize) / 64;
        let bit_index = (value as usize) % 64;
        let mask = 1u64 << bit_index;

        let old = self.bits[word_index] & mask;
        self.bits[word_index] |= mask;
        old == 0
    }

    fn remove(&mut self, value: u16) -> bool {
        let word_index = (value as usize) / 64;
        let bit_index = (value as usize) % 64;
        let mask = 1u64 << bit_index;

        let old = self.bits[word_index] & mask;
        self.bits[word_index] &= !mask;
        old != 0
    }

    fn contains(&self, value: u16) -> bool {
        let word_index = (value as usize) / 64;
        let bit_index = (value as usize) % 64;
        let mask = 1u64 << bit_index;

        (self.bits[word_index] & mask) != 0
    }

    fn cardinality(&self) -> u32 {
        self.bits.iter().map(|word| word.count_ones()).sum()
    }

    fn union(&self, other: &BitmapContainer) -> BitmapContainer {
        let mut result = BitmapContainer::new();
        for i in 0..1024 {
            result.bits[i] = self.bits[i] | other.bits[i];
        }
        result
    }

    fn intersect(&self, other: &BitmapContainer) -> BitmapContainer {
        let mut result = BitmapContainer::new();
        for i in 0..1024 {
            result.bits[i] = self.bits[i] & other.bits[i];
        }
        result
    }
}

/// Run container for consecutive ranges (simplified implementation)
#[derive(Debug, Clone)]
struct RunContainer {
    runs: Vec<(u16, u16)>, // (start, length) pairs
}

impl RunContainer {
    fn new() -> Self {
        RunContainer { runs: Vec::new() }
    }

    fn insert(&mut self, value: u16) -> bool {
        // Simplified implementation - convert to individual elements
        for &(start, length) in &self.runs {
            if value >= start && value < start + length {
                return false; // Already exists
            }
        }

        // Add as single element run
        self.runs.push((value, 1));
        self.runs.sort_by_key(|(start, _)| *start);
        self.merge_adjacent_runs();
        true
    }

    fn remove(&mut self, value: u16) -> bool {
        // Simplified implementation
        let mut found = false;
        let mut new_runs = Vec::new();

        for (start, length) in &self.runs {
            if value >= *start && value < start + length {
                found = true;
                // Split the run if necessary
                if value > *start {
                    new_runs.push((*start, value - start));
                }
                if value + 1 < start + length {
                    new_runs.push((value + 1, start + length - value - 1));
                }
            } else {
                new_runs.push((*start, *length));
            }
        }

        self.runs = new_runs;
        found
    }

    fn contains(&self, value: u16) -> bool {
        for &(start, length) in &self.runs {
            if value >= start && value < start + length {
                return true;
            }
        }
        false
    }

    fn cardinality(&self) -> u32 {
        self.runs.iter().map(|(_, length)| *length as u32).sum()
    }

    fn iter(&self) -> RunIterator {
        RunIterator {
            runs: &self.runs,
            run_index: 0,
            value_offset: 0,
        }
    }

    fn merge_adjacent_runs(&mut self) {
        if self.runs.len() <= 1 {
            return;
        }

        let mut merged = Vec::new();
        let mut current_start = self.runs[0].0;
        let mut current_end = self.runs[0].0 + self.runs[0].1;

        for i in 1..self.runs.len() {
            let (start, length) = self.runs[i];
            let end = start + length;

            if start <= current_end {
                // Merge runs
                current_end = current_end.max(end);
            } else {
                // Add previous run and start new one
                merged.push((current_start, current_end - current_start));
                current_start = start;
                current_end = end;
            }
        }

        merged.push((current_start, current_end - current_start));
        self.runs = merged;
    }
}

/// Iterator for run container
struct RunIterator<'a> {
    runs: &'a Vec<(u16, u16)>,
    run_index: usize,
    value_offset: u16,
}

impl<'a> Iterator for RunIterator<'a> {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        while self.run_index < self.runs.len() {
            let (start, length) = self.runs[self.run_index];

            if self.value_offset < length {
                let value = start + self.value_offset;
                self.value_offset += 1;
                return Some(value);
            } else {
                self.run_index += 1;
                self.value_offset = 0;
            }
        }

        None
    }
}

/// Iterator for bitmap container
struct BitmapIterator<'a> {
    bits: &'a [u64; 1024],
    word_index: usize,
    current_word: u64,
    bit_offset: u32,
}

impl<'a> BitmapIterator<'a> {
    fn new(bits: &'a [u64; 1024]) -> Self {
        let mut iter = BitmapIterator {
            bits,
            word_index: 0,
            current_word: bits[0],
            bit_offset: 0,
        };

        // Skip to first set bit
        iter.advance_to_next_bit();
        iter
    }

    fn advance_to_next_bit(&mut self) {
        while self.word_index < 1024 {
            if self.current_word != 0 {
                // Find next set bit in current word
                while self.bit_offset < 64 {
                    if (self.current_word & (1u64 << self.bit_offset)) != 0 {
                        return;
                    }
                    self.bit_offset += 1;
                }
            }

            // Move to next word
            self.word_index += 1;
            self.bit_offset = 0;

            if self.word_index < 1024 {
                self.current_word = self.bits[self.word_index];
            }
        }
    }
}

impl<'a> Iterator for BitmapIterator<'a> {
    type Item = u16;

    fn next(&mut self) -> Option<Self::Item> {
        if self.word_index >= 1024 {
            return None;
        }

        let value = (self.word_index * 64 + self.bit_offset as usize) as u16;
        self.bit_offset += 1;
        self.advance_to_next_bit();

        Some(value)
    }
}

/// Main Roaring bitmap structure
#[derive(Debug, Clone)]
pub struct RoaringBitmap {
    /// Map from high 16 bits to container
    containers: BTreeMap<u16, Container>,
}

impl RoaringBitmap {
    /// Create a new empty Roaring bitmap
    pub fn new() -> Self {
        RoaringBitmap {
            containers: BTreeMap::new(),
        }
    }

    /// Insert a value into the bitmap
    pub fn insert(&mut self, value: u32) -> bool {
        let high = (value >> 16) as u16;
        let low = value as u16;

        let container = self
            .containers
            .entry(high)
            .or_insert_with(|| Container::Array(ArrayContainer::new()));

        let inserted = container.insert(low);

        // Optimize container type if necessary
        if inserted {
            let optimized = std::mem::replace(container, Container::Array(ArrayContainer::new()));
            *container = optimized.optimize();
        }

        inserted
    }

    /// Remove a value from the bitmap
    pub fn remove(&mut self, value: u32) -> bool {
        let high = (value >> 16) as u16;
        let low = value as u16;

        if let Some(container) = self.containers.get_mut(&high) {
            let removed = container.remove(low);

            // Remove container if empty
            if removed && container.cardinality() == 0 {
                self.containers.remove(&high);
            } else if removed {
                // Optimize container type
                let optimized =
                    std::mem::replace(container, Container::Array(ArrayContainer::new()));
                *container = optimized.optimize();
            }

            removed
        } else {
            false
        }
    }

    /// Check if bitmap contains a value
    pub fn contains(&self, value: u32) -> bool {
        let high = (value >> 16) as u16;
        let low = value as u16;

        self.containers
            .get(&high)
            .map_or(false, |container| container.contains(low))
    }

    /// Get the number of elements in the bitmap
    pub fn cardinality(&self) -> u64 {
        self.containers
            .values()
            .map(|container| container.cardinality() as u64)
            .sum()
    }

    /// Check if bitmap is empty
    pub fn is_empty(&self) -> bool {
        self.containers.is_empty()
    }

    /// Clear all elements from the bitmap
    pub fn clear(&mut self) {
        self.containers.clear();
    }

    /// Union with another bitmap
    pub fn union(&self, other: &RoaringBitmap) -> RoaringBitmap {
        let mut result = RoaringBitmap::new();

        // Get all unique high values
        let mut high_values: std::collections::BTreeSet<u16> = std::collections::BTreeSet::new();
        high_values.extend(self.containers.keys());
        high_values.extend(other.containers.keys());

        for high in high_values {
            match (self.containers.get(&high), other.containers.get(&high)) {
                (Some(c1), Some(c2)) => {
                    result.containers.insert(high, c1.union(c2));
                }
                (Some(c1), None) => {
                    result.containers.insert(high, c1.clone());
                }
                (None, Some(c2)) => {
                    result.containers.insert(high, c2.clone());
                }
                (None, None) => unreachable!(),
            }
        }

        result
    }

    /// Intersection with another bitmap
    pub fn intersect(&self, other: &RoaringBitmap) -> RoaringBitmap {
        let mut result = RoaringBitmap::new();

        for (high, container1) in &self.containers {
            if let Some(container2) = other.containers.get(high) {
                let intersection = container1.intersect(container2);
                if intersection.cardinality() > 0 {
                    result.containers.insert(*high, intersection);
                }
            }
        }

        result
    }

    /// Difference with another bitmap (self - other)
    pub fn difference(&self, other: &RoaringBitmap) -> RoaringBitmap {
        let mut result = self.clone();

        for (high, other_container) in &other.containers {
            if let Some(self_container) = result.containers.get_mut(high) {
                // Remove elements that exist in other
                for value in other_container.iter() {
                    self_container.remove(value);
                }

                // Remove container if empty
                if self_container.cardinality() == 0 {
                    result.containers.remove(high);
                } else {
                    // Optimize container
                    let optimized =
                        std::mem::replace(self_container, Container::Array(ArrayContainer::new()));
                    *self_container = optimized.optimize();
                }
            }
        }

        result
    }

    /// XOR with another bitmap
    pub fn xor(&self, other: &RoaringBitmap) -> RoaringBitmap {
        // XOR = (A ∪ B) - (A ∩ B)
        let union = self.union(other);
        let intersection = self.intersect(other);
        union.difference(&intersection)
    }

    /// Get iterator over all values in the bitmap
    pub fn iter(&self) -> BitmapIteratorAll {
        BitmapIteratorAll {
            containers_iter: self.containers.iter(),
            current_container: None,
            current_high: 0,
        }
    }

    /// Convert to vector (for small bitmaps)
    pub fn to_vec(&self) -> Vec<u32> {
        self.iter().collect()
    }

    /// Create bitmap from iterator
    pub fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = u32>,
    {
        let mut bitmap = RoaringBitmap::new();
        for value in iter {
            bitmap.insert(value);
        }
        bitmap
    }

    /// Get range of values [start, end)
    pub fn range(&self, start: u32, end: u32) -> RoaringBitmap {
        let mut result = RoaringBitmap::new();

        for value in self.iter() {
            if value >= start && value < end {
                result.insert(value);
            } else if value >= end {
                break;
            }
        }

        result
    }

    /// Serialize to bytes
    pub fn serialize(&self) -> Result<Vec<u8>, BitmapError> {
        let mut bytes = Vec::new();

        // Write number of containers
        let container_count = self.containers.len() as u32;
        bytes.extend_from_slice(&container_count.to_le_bytes());

        for (high, container) in &self.containers {
            // Write high value
            bytes.extend_from_slice(&high.to_le_bytes());

            // Write container type and data
            match container {
                Container::Array(array) => {
                    bytes.push(0); // Array container type
                    let count = array.values.len() as u16;
                    bytes.extend_from_slice(&count.to_le_bytes());
                    for &value in &array.values {
                        bytes.extend_from_slice(&value.to_le_bytes());
                    }
                }
                Container::Bitmap(bitmap) => {
                    bytes.push(1); // Bitmap container type
                    for &word in &bitmap.bits {
                        bytes.extend_from_slice(&word.to_le_bytes());
                    }
                }
                Container::Run(_) => {
                    bytes.push(2); // Run container type (simplified)
                    // TODO: Implement run container serialization
                }
            }
        }

        Ok(bytes)
    }

    /// Deserialize from bytes
    pub fn deserialize(bytes: &[u8]) -> Result<Self, BitmapError> {
        if bytes.len() < 4 {
            return Err(BitmapError::SerializationError(
                "Invalid data length".to_string(),
            ));
        }

        let mut bitmap = RoaringBitmap::new();
        let mut offset = 0;

        // Read container count
        let container_count = u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        offset += 4;

        for _ in 0..container_count {
            if offset + 3 >= bytes.len() {
                return Err(BitmapError::SerializationError(
                    "Unexpected end of data".to_string(),
                ));
            }

            // Read high value
            let high = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
            offset += 2;

            // Read container type
            let container_type = bytes[offset];
            offset += 1;

            let container = match container_type {
                0 => {
                    // Array container
                    if offset + 2 >= bytes.len() {
                        return Err(BitmapError::SerializationError(
                            "Invalid array container".to_string(),
                        ));
                    }

                    let count = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                    offset += 2;

                    let mut array = ArrayContainer::new();
                    for _ in 0..count {
                        if offset + 2 > bytes.len() {
                            return Err(BitmapError::SerializationError(
                                "Invalid array data".to_string(),
                            ));
                        }

                        let value = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                        array.insert(value);
                        offset += 2;
                    }

                    Container::Array(array)
                }
                1 => {
                    // Bitmap container
                    if offset + 8192 > bytes.len() {
                        // 1024 * 8 bytes
                        return Err(BitmapError::SerializationError(
                            "Invalid bitmap container".to_string(),
                        ));
                    }

                    let mut bitmap = BitmapContainer::new();
                    for i in 0..1024 {
                        let word_bytes = &bytes[offset..offset + 8];
                        bitmap.bits[i] = u64::from_le_bytes([
                            word_bytes[0],
                            word_bytes[1],
                            word_bytes[2],
                            word_bytes[3],
                            word_bytes[4],
                            word_bytes[5],
                            word_bytes[6],
                            word_bytes[7],
                        ]);
                        offset += 8;
                    }

                    Container::Bitmap(bitmap)
                }
                _ => {
                    return Err(BitmapError::SerializationError(
                        "Unknown container type".to_string(),
                    ));
                }
            };

            bitmap.containers.insert(high, container);
        }

        Ok(bitmap)
    }

    /// Get the serialized size of the bitmap in bytes
    pub fn serialized_size(&self) -> usize {
        // Header: 4 bytes for cookie, 4 bytes for container count
        let mut size = 8;

        // For each container: 2 bytes for key, 2 bytes for cardinality minus 1, container data
        for (_, container) in &self.containers {
            size += 4; // key + cardinality

            match container {
                Container::Array(array) => {
                    // Array container: 2 bytes per value
                    size += array.len() * 2;
                }
                Container::Bitmap(_bitmap) => {
                    // Bitmap container: always 8KB (65536 bits / 8)
                    size += 8192;
                }
                Container::Run(run) => {
                    // Run container: 2 bytes per run (start + length)
                    size += run.runs.len() * 4;
                }
            }
        }

        size
    }

    /// Compute the intersection with another bitmap (alias for intersect)
    pub fn and(&self, other: &RoaringBitmap) -> RoaringBitmap {
        self.intersect(other)
    }

    /// Compute the union with another bitmap (alias for union)
    pub fn or(&self, other: &RoaringBitmap) -> RoaringBitmap {
        self.union(other)
    }
}

impl Default for RoaringBitmap {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::BitAndAssign<&RoaringBitmap> for RoaringBitmap {
    fn bitand_assign(&mut self, rhs: &RoaringBitmap) {
        *self = self.and(rhs);
    }
}

impl std::ops::BitOrAssign<&RoaringBitmap> for RoaringBitmap {
    fn bitor_assign(&mut self, rhs: &RoaringBitmap) {
        *self = self.or(rhs);
    }
}

impl std::ops::SubAssign<&RoaringBitmap> for RoaringBitmap {
    fn sub_assign(&mut self, rhs: &RoaringBitmap) {
        *self = self.difference(rhs);
    }
}

/// Iterator over all values in a RoaringBitmap
pub struct BitmapIteratorAll<'a> {
    containers_iter: std::collections::btree_map::Iter<'a, u16, Container>,
    current_container: Option<(u16, Box<dyn Iterator<Item = u16> + 'a>)>,
    current_high: u16,
}

impl<'a> Iterator for BitmapIteratorAll<'a> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((high, ref mut iter)) = self.current_container {
                if let Some(low) = iter.next() {
                    return Some(((high as u32) << 16) | (low as u32));
                } else {
                    self.current_container = None;
                }
            }

            if let Some((high, container)) = self.containers_iter.next() {
                self.current_container = Some((*high, container.iter()));
            } else {
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut bitmap = RoaringBitmap::new();

        assert!(bitmap.is_empty());
        assert_eq!(bitmap.cardinality(), 0);

        // Insert values
        assert!(bitmap.insert(1));
        assert!(bitmap.insert(100));
        assert!(bitmap.insert(1000));
        assert!(!bitmap.insert(1)); // Already exists

        assert_eq!(bitmap.cardinality(), 3);
        assert!(bitmap.contains(1));
        assert!(bitmap.contains(100));
        assert!(bitmap.contains(1000));
        assert!(!bitmap.contains(2));
    }

    #[test]
    fn test_remove() {
        let mut bitmap = RoaringBitmap::new();

        bitmap.insert(1);
        bitmap.insert(2);
        bitmap.insert(3);

        assert!(bitmap.remove(2));
        assert!(!bitmap.remove(2)); // Already removed
        assert!(!bitmap.contains(2));
        assert_eq!(bitmap.cardinality(), 2);
    }

    #[test]
    fn test_union() {
        let mut bitmap1 = RoaringBitmap::new();
        bitmap1.insert(1);
        bitmap1.insert(2);
        bitmap1.insert(3);

        let mut bitmap2 = RoaringBitmap::new();
        bitmap2.insert(2);
        bitmap2.insert(3);
        bitmap2.insert(4);

        let union = bitmap1.union(&bitmap2);
        assert_eq!(union.cardinality(), 4);
        assert!(union.contains(1));
        assert!(union.contains(2));
        assert!(union.contains(3));
        assert!(union.contains(4));
    }

    #[test]
    fn test_intersection() {
        let mut bitmap1 = RoaringBitmap::new();
        bitmap1.insert(1);
        bitmap1.insert(2);
        bitmap1.insert(3);

        let mut bitmap2 = RoaringBitmap::new();
        bitmap2.insert(2);
        bitmap2.insert(3);
        bitmap2.insert(4);

        let intersection = bitmap1.intersect(&bitmap2);
        assert_eq!(intersection.cardinality(), 2);
        assert!(!intersection.contains(1));
        assert!(intersection.contains(2));
        assert!(intersection.contains(3));
        assert!(!intersection.contains(4));
    }

    #[test]
    fn test_difference() {
        let mut bitmap1 = RoaringBitmap::new();
        bitmap1.insert(1);
        bitmap1.insert(2);
        bitmap1.insert(3);

        let mut bitmap2 = RoaringBitmap::new();
        bitmap2.insert(2);
        bitmap2.insert(4);

        let difference = bitmap1.difference(&bitmap2);
        assert_eq!(difference.cardinality(), 2);
        assert!(difference.contains(1));
        assert!(!difference.contains(2));
        assert!(difference.contains(3));
        assert!(!difference.contains(4));
    }

    #[test]
    fn test_xor() {
        let mut bitmap1 = RoaringBitmap::new();
        bitmap1.insert(1);
        bitmap1.insert(2);
        bitmap1.insert(3);

        let mut bitmap2 = RoaringBitmap::new();
        bitmap2.insert(2);
        bitmap2.insert(3);
        bitmap2.insert(4);

        let xor = bitmap1.xor(&bitmap2);
        assert_eq!(xor.cardinality(), 2);
        assert!(xor.contains(1));
        assert!(!xor.contains(2));
        assert!(!xor.contains(3));
        assert!(xor.contains(4));
    }

    #[test]
    fn test_large_values() {
        let mut bitmap = RoaringBitmap::new();

        // Test values that span multiple containers
        bitmap.insert(0);
        bitmap.insert(65536); // Different high 16 bits
        bitmap.insert(131072); // Different high 16 bits
        bitmap.insert(4294967295); // Max u32

        assert_eq!(bitmap.cardinality(), 4);
        assert!(bitmap.contains(0));
        assert!(bitmap.contains(65536));
        assert!(bitmap.contains(131072));
        assert!(bitmap.contains(4294967295));
    }

    #[test]
    fn test_dense_data() {
        let mut bitmap = RoaringBitmap::new();

        // Insert many consecutive values to trigger bitmap container
        for i in 0..5000 {
            bitmap.insert(i);
        }

        assert_eq!(bitmap.cardinality(), 5000);

        for i in 0..5000 {
            assert!(bitmap.contains(i));
        }

        assert!(!bitmap.contains(5000));
    }

    #[test]
    fn test_iterator() {
        let mut bitmap = RoaringBitmap::new();

        let values = vec![1, 5, 10, 100, 1000];
        for &value in &values {
            bitmap.insert(value);
        }

        let collected: Vec<u32> = bitmap.iter().collect();
        assert_eq!(collected, values);
    }

    #[test]
    fn test_from_iter() {
        let values = vec![3, 1, 4, 1, 5, 9, 2, 6];
        let bitmap = RoaringBitmap::from_iter(values);

        let mut expected = vec![1, 2, 3, 4, 5, 6, 9];
        expected.sort();

        let mut collected: Vec<u32> = bitmap.iter().collect();
        collected.sort();

        assert_eq!(collected, expected);
    }

    #[test]
    fn test_range() {
        let mut bitmap = RoaringBitmap::new();

        for i in 0..100 {
            bitmap.insert(i);
        }

        let range = bitmap.range(10, 20);
        assert_eq!(range.cardinality(), 10);

        for i in 10..20 {
            assert!(range.contains(i));
        }

        assert!(!range.contains(9));
        assert!(!range.contains(20));
    }

    #[test]
    fn test_serialization() {
        let mut original = RoaringBitmap::new();
        original.insert(1);
        original.insert(100);
        original.insert(10000);

        let bytes = original.serialize().unwrap();
        let deserialized = RoaringBitmap::deserialize(&bytes).unwrap();

        assert_eq!(original.cardinality(), deserialized.cardinality());
        assert!(deserialized.contains(1));
        assert!(deserialized.contains(100));
        assert!(deserialized.contains(10000));
    }

    #[test]
    fn test_clear() {
        let mut bitmap = RoaringBitmap::new();
        bitmap.insert(1);
        bitmap.insert(2);
        bitmap.insert(3);

        assert_eq!(bitmap.cardinality(), 3);

        bitmap.clear();

        assert!(bitmap.is_empty());
        assert_eq!(bitmap.cardinality(), 0);
    }

    #[test]
    fn test_container_optimization() {
        let mut bitmap = RoaringBitmap::new();

        // Start with sparse data (should use array container)
        for i in 0..10 {
            bitmap.insert(i * 100);
        }

        // Add dense data to trigger bitmap container
        for i in 0..5000 {
            bitmap.insert(i);
        }

        assert!(bitmap.cardinality() > 4096);

        // Remove most elements to trigger conversion back to array
        for i in 100..5000 {
            bitmap.remove(i);
        }

        assert!(bitmap.cardinality() < 4096);
    }
}
