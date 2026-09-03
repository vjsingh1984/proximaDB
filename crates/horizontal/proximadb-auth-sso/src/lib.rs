// TD-SSO-1: dead crate pending removal — zero production callers.
#![allow(dead_code, unused_imports, unused_variables, unreachable_code)]
// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Enterprise SSO / federated-identity integrations, extracted from the root
//! `auth/sso` module (TD-DECOMP-21).
//!
//! [`sso`] holds the provider integrations (AWS IAM, Azure AD, Google Cloud,
//! SAML) and the shared enterprise-identity types they produce. The subsystem
//! is pure logic over `anyhow`/`chrono`/`serde`/`tracing` (no cloud SDKs), which
//! keeps it a clean horizontal-tier leaf and lets its 139 inline tests run in
//! this crate's own binary rather than the root lib's.

pub mod sso;
