// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! # FA enforcement substrate — the TD-FOUNDATION-3 preconditions
//!
//! TD-FOUNDATION-3 (the ABAC **enforcement** track, "FA") is gated on four
//! preconditions, three of which are *subsystems that do not exist today*. This
//! crate is those three, built ahead of the enforcement wiring so FA is startable:
//!
//! | P | Precondition | Module |
//! |---|---|---|
//! | **P1** | Attribute-authority store — a first-class `(subject_id, tenant_stable_id) → {roles, attrs}` binding. Today's `user_role_assignments` is `user_id`-keyed, single-assignment, permission-only; ABAC attributes have **no** server-side authority. | [`authority`] |
//! | **P3** | Subject + policy-epoch cache keying, so no client-servable cache can hand subject A's rows to subject B (TF-2 S10). | [`cache_key`] |
//! | **P4** | A **non-`Option`** read context — the type that makes "unfiltered by default" unrepresentable (TF-2 S4/S7). | [`context`] |
//!
//! P2 (live JWT signature verification) is not here: it is an independent
//! security defect on the whole auth boundary, tracked on its own security TD.
//!
//! ## Status: substrate only, wired into nothing
//!
//! Nothing in the workspace depends on this crate. It defines the types and the
//! traits FA will attach at the read primitives; it performs **no enforcement**,
//! reads no storage, and is on no request path. (It is deliberately *not*
//! `#[cfg]`-gated out of the default build: a security substrate that never
//! compiles is a security substrate that is never tested. Inertness comes from
//! having no reverse dependency, which is checkable; the `abac-policy` feature
//! gates the *wiring*, in FA.)
//!
//! ## The three properties everything here exists to make structural
//!
//! 1. **Fail-closed is the type's default, not the caller's discipline.** There
//!    is no `AuthorizedReadContext::default()`, no `unfiltered()` constructor, and
//!    no `Option<Context>` — resolution yields [`context::ReadDecision`], and the
//!    deny arm carries no context at all. An unresolvable subject, an absent
//!    binding, and a store outage all deny.
//! 2. **Claims are never load-bearing.** Only [`authority::AttributeAuthority`]
//!    mints `ServerResolved` attributes, and only from a server-side binding. A
//!    forged `clearance`/`role` claim cannot reach an admit/deny decision because
//!    the value a predicate reads comes from the authority, not the token.
//! 3. **A `SystemReadContext` cannot serve a client.** Unfiltered internal reads
//!    (compaction, index build, recovery) are explicit and audited, and the
//!    [`context::ClientServable`] trait they do **not** implement is what stops
//!    them reaching a client sink — at compile time, not by review.
//!
//! Design of record: `docs/10-quality/td/TD-FOUNDATION-3-abac-enforcement-subsystem.adoc`
//! (search for 'TD-FOUNDATION-3' or 'ABAC enforcement' to find the full track).
//! The enforcement wiring plan: `docs/12-design/` + the 5-phase plan.
//! (build track) and `TD-FOUNDATION-2` §3.4 / §8 (architecture + S1–S10).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod authority;
pub mod cache_key;
pub mod compile;
pub mod context;

pub use authority::{
    AttributeAuthority, AttributeBinding, AttributeDigest, AuthorityError,
    InMemoryAttributeAuthority, ResolvedSubject,
};
pub use cache_key::{InMemoryPolicyEpochs, PolicyEpoch, PolicyEpochSource, SubjectCacheKey};
pub use compile::{InMemoryPredicateObjectStore, PredicateObjectStore, compile_security_filter};
pub use context::{
    AuthorizedReadContext, ClientServable, ColumnDecision, DenyReason, ForbiddenColumn,
    ReadDecision, SystemReadContext, SystemReadReason,
};
