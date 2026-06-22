// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metadata Storage Backends
//!
//! The legacy collection-metadata backends (`UniversalMetadataBackend`,
//! `LocalRocksDbBackend`) and their `MetadataBackendFactory` have been removed:
//! the system catalog (`xCatalog`) is now the sole authoritative store for
//! collection metadata. This module is retained only as a namespace placeholder
//! and will be deleted once its parent module no longer declares it.
