/*
 * Copyright 2026 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Governance and security modules
//!
//! This module provides access control, RBAC, and other governance features.

pub mod collection_rbac;

pub use collection_rbac::{CollectionRbacExt, RbacError, check_collection_access};
