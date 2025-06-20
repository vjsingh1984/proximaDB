/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Protocol buffer definitions and generated code
//!
//! This module contains the generated protobuf code for ProximaDB's gRPC API.

// New unified protobuf v1 protocol
pub mod proximadb {
    include!("proximadb.rs");
}

// Moved to obsolete directory - not currently used in gRPC service
// pub mod proximadb_avro {
//     include!("proximadb.avro.rs");  // MOVED: obsolete/proto/proximadb.avro.rs
// }

/// File descriptor set for gRPC reflection
/// Note: This will be generated during build - for now we'll use an empty placeholder
pub const FILE_DESCRIPTOR_SET: &[u8] = &[];
