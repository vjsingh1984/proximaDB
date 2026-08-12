// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! P4 — the non-`Option` read context (TF-2 S4/S7).
//!
//! Today's shape is `tenant_context: Option<&TenantContext>` with a permissive
//! default body on `scan_records` — i.e. *unfiltered* is what you get by writing
//! less code. FA inverts that: [`AuthorizedReadContext`] is a **required**
//! parameter of every read primitive, and the type offers no way to produce one
//! that means "no restriction".
//!
//! * There is no `Default`, no `unfiltered()`, and no `new()` taking a policy you
//!   chose yourself. The only constructor is [`AuthorizedReadContext::resolve`],
//!   which returns a [`ReadDecision`] — and the deny arm carries **no context at
//!   all**, so a denied read has nothing to pass to a primitive.
//! * Internal callers that genuinely need unfiltered access (compaction, index
//!   build, recovery, replication) use [`SystemReadContext`], which names its
//!   reason and, crucially, does **not** implement [`ClientServable`]. A sink that
//!   serves clients accepts `impl ClientServable`; handing it a
//!   `SystemReadContext` does not compile.
//!
//! What is deliberately **not** here: the compiled row filter. Turning
//! [`AuthorizedReadContext::row_predicate_refs`] into an executable predicate is
//! the total-or-fail-closed bridge (FA-2 / TF-2 S1–S3), which lives against the
//! filter-expression crates. This module carries the refs and the field masks;
//! FA-2 adds the compiled filter to the same context.

use std::collections::BTreeMap;

use proximadb_catalog::fc_metamodel::{
    Effect, EffectivePolicy, FieldMask, NamespaceId, ObjectId, PolicyBinding, Scope,
    SubjectAttributes, SubjectId, Target, TenantStableId, resolve_effective_policy,
};

use crate::authority::{AttributeAuthority, AttributeDigest, AuthorityError};
use crate::cache_key::{PolicyEpoch, PolicyEpochSource, SubjectCacheKey};

/// Why a read was denied. Every variant means **zero rows**, never "unfiltered".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DenyReason {
    /// The subject's attributes could not be resolved server-side. Note this is
    /// a deny even though the *policy* might have permitted: a policy evaluated
    /// against unresolved attributes is not evaluated at all.
    #[error("subject attributes could not be resolved: {0}")]
    AttributeResolution(#[from] AuthorityError),
    /// No binding covers the target. The fail-closed default — an unconfigured
    /// container is closed, not open.
    #[error("no policy binding applies to the target; default is deny")]
    NoApplicablePolicy,
    /// A binding at some scope explicitly denied (deny wins over any permit).
    #[error("an applicable policy binding denies this read")]
    ExplicitDeny,
    /// ADR-090 L1.2: grant enforcement is armed and no applicable grant admits
    /// the subject to the target. Fail-closed: absence of entitlement is deny.
    #[error("no applicable grant admits this subject to the target")]
    NoApplicableGrant,
}

/// The outcome of resolving a read: an admitted context, or a reason and nothing
/// else.
///
/// The shape is the point. A `Result<Option<Context>, E>` would let a caller
/// `unwrap_or(None)` its way back to unfiltered; here the deny arm simply has no
/// context to unwrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadDecision {
    /// The read may proceed under this context.
    Admit(AuthorizedReadContext),
    /// The read is denied.
    Deny(DenyReason),
}

impl ReadDecision {
    /// The context, if admitted.
    pub fn admitted(self) -> Option<AuthorizedReadContext> {
        match self {
            ReadDecision::Admit(ctx) => Some(ctx),
            ReadDecision::Deny(_) => None,
        }
    }

    /// Whether this decision denied.
    pub fn is_deny(&self) -> bool {
        matches!(self, ReadDecision::Deny(_))
    }
}

/// What a subject may see of one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ColumnDecision {
    /// Returned as stored.
    Visible,
    /// Returned as NULL.
    Null,
    /// Returned masked.
    Redact,
    /// Not returnable at all — the **query is rejected**, not silently nulled
    /// (TF-2 §3.4.7: a null projection would leak the column's existence and let
    /// a caller distinguish "masked" from "absent").
    Forbidden,
}

impl ColumnDecision {
    fn of(mask: FieldMask) -> Self {
        match mask {
            FieldMask::Null => ColumnDecision::Null,
            FieldMask::Redact => ColumnDecision::Redact,
            FieldMask::Forbid => ColumnDecision::Forbidden,
        }
    }
}

/// A projection was rejected because it referenced a forbidden column.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("column {ordinal} is forbidden to this subject; the query is rejected")]
pub struct ForbiddenColumn {
    /// The offending column ordinal.
    pub ordinal: u32,
}

/// The mandatory context every client-serving read primitive takes.
///
/// Construct it only via [`AuthorizedReadContext::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedReadContext {
    subject: SubjectAttributes,
    digest: AttributeDigest,
    target: Target,
    namespace: NamespaceId,
    tenant_stable_id: TenantStableId,
    policy: EffectivePolicy,
    epoch: PolicyEpoch,
    column_decisions: BTreeMap<u32, ColumnDecision>,
}

impl AuthorizedReadContext {
    /// Resolve a subject's read authorization for `target`.
    ///
    /// Order matters and is fail-closed at each step:
    ///
    /// 1. Resolve the subject's attributes **server-side** (P1). A failure denies
    ///    before any policy is consulted — claims never substitute.
    /// 2. Compose the applicable bindings deny-biased (FC's
    ///    `resolve_effective_policy`). No applicable binding denies.
    /// 3. Collect column decisions from column-scoped bindings, strictest-wins.
    ///
    /// `target.column` is ignored for the row decision — the context is resolved
    /// at table granularity and answers per-column questions through
    /// [`Self::column_decision`].
    pub fn resolve(
        authority: &dyn AttributeAuthority,
        epochs: &dyn PolicyEpochSource,
        bindings: &[PolicyBinding],
        subject_id: &SubjectId,
        tenant_stable_id: TenantStableId,
        target: Target,
    ) -> ReadDecision {
        let resolved = match authority.resolve_effective_attributes(subject_id, tenant_stable_id) {
            Ok(r) => r,
            Err(e) => return ReadDecision::Deny(DenyReason::AttributeResolution(e)),
        };

        // Only this tenant's bindings may govern this tenant's read. Isolation is
        // structural: a foreign binding is filtered out here, never compared.
        let own: Vec<PolicyBinding> = bindings
            .iter()
            .filter(|b| b.tenant_stable_id == tenant_stable_id)
            .cloned()
            .collect();

        let row_target = Target {
            column: None,
            ..target
        };
        let policy = resolve_effective_policy(&own, &row_target);
        if policy.applicable == 0 {
            return ReadDecision::Deny(DenyReason::NoApplicablePolicy);
        }
        if policy.decision == Effect::Deny {
            return ReadDecision::Deny(DenyReason::ExplicitDeny);
        }

        let mut column_decisions: BTreeMap<u32, ColumnDecision> = BTreeMap::new();
        for b in &own {
            let Scope::Column { table, column } = b.scope else {
                continue;
            };
            if table != target.table {
                continue;
            }
            // A column-scoped Deny forbids the column outright, whether or not it
            // also carries a mask — the strictest reading of an explicit deny.
            let decision = if b.effect == Effect::Deny {
                ColumnDecision::Forbidden
            } else {
                match b.field_mask {
                    Some(m) => ColumnDecision::of(m),
                    None => continue,
                }
            };
            let slot = column_decisions.entry(column).or_insert(decision);
            // Strictest wins when two bindings mask the same column
            // (Forbidden > Redact > Null > Visible, per the enum's order).
            *slot = (*slot).max(decision);
        }

        ReadDecision::Admit(AuthorizedReadContext {
            subject: resolved.attributes,
            digest: resolved.digest,
            target: row_target,
            namespace: target.namespace,
            tenant_stable_id,
            policy,
            epoch: epochs.epoch(tenant_stable_id, target.namespace),
            column_decisions,
        })
    }

    /// The subject's server-resolved attributes. A predicate reads values through
    /// `SubjectAttributes::load_bearing`, which refuses claim-sourced values.
    pub fn subject(&self) -> &SubjectAttributes {
        &self.subject
    }

    /// The container this context authorizes reads of.
    pub fn target(&self) -> Target {
        self.target
    }

    /// The tenant this read is scoped to.
    pub fn tenant_stable_id(&self) -> TenantStableId {
        self.tenant_stable_id
    }

    /// The row-predicate object refs to AND into the scan. FA-2 compiles these
    /// through the total-or-fail-closed bridge; an empty list means the applicable
    /// permits carried no row restriction, **not** that the read is unauthorized —
    /// authorization already happened.
    pub fn row_predicate_refs(&self) -> &[ObjectId] {
        &self.policy.predicate_refs
    }

    /// What the subject may see of one column. Unmentioned columns are
    /// [`ColumnDecision::Visible`] — the row-level decision already admitted the
    /// row, and masks are the exception, not the rule.
    pub fn column_decision(&self, ordinal: u32) -> ColumnDecision {
        self.column_decisions
            .get(&ordinal)
            .copied()
            .unwrap_or(ColumnDecision::Visible)
    }

    /// The columns carrying any mask.
    ///
    /// FA's policy-compile step uses this for TF-2 S8: a masked column must not
    /// be referenced by a row predicate, or its value leaks through the admit/deny
    /// channel despite the output mask.
    pub fn masked_columns(&self) -> impl Iterator<Item = (u32, ColumnDecision)> + '_ {
        self.column_decisions.iter().map(|(k, v)| (*k, *v))
    }

    /// Decide a projection. A forbidden column **rejects the query** rather than
    /// nulling itself out.
    pub fn project(&self, columns: &[u32]) -> Result<Vec<(u32, ColumnDecision)>, ForbiddenColumn> {
        let mut out = Vec::with_capacity(columns.len());
        for &ordinal in columns {
            let decision = self.column_decision(ordinal);
            if decision == ColumnDecision::Forbidden {
                return Err(ForbiddenColumn { ordinal });
            }
            out.push((ordinal, decision));
        }
        Ok(out)
    }

    /// The key component any client-servable cache must fold in (P3 / TF-2 S10).
    pub fn subject_cache_key(&self) -> SubjectCacheKey {
        SubjectCacheKey::new(
            self.tenant_stable_id,
            self.namespace,
            self.epoch,
            self.digest,
        )
    }
}

/// Why an unfiltered internal read is legitimate. Naming the reason is what makes
/// [`SystemReadContext`] auditable rather than a blanket escape hatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemReadReason {
    /// Compaction / merge rewriting segments.
    Compaction,
    /// Building or rebuilding an index.
    IndexBuild,
    /// Resolving a foreign-key target during constraint checking.
    ForeignKeyResolution,
    /// Refreshing a materialized view.
    MaterializedViewRefresh,
    /// Node-to-node replication of storage state.
    Replication,
    /// WAL recovery / restart warming.
    Recovery,
    /// Emitting internal statistics or metrics.
    Statistics,
}

/// An explicit, audited unfiltered read, for internal machinery that has no
/// subject to filter by.
///
/// It carries no subject, produces no cache key, and — deliberately — does **not**
/// implement [`ClientServable`]. That omission is the enforcement: a sink that
/// serves clients is written against `impl ClientServable`, so handing it a
/// `SystemReadContext` fails to compile rather than failing in review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemReadContext {
    reason: SystemReadReason,
    /// Free-text provenance for the audit log — typically the calling subsystem.
    origin: String,
}

impl SystemReadContext {
    /// Declare an unfiltered internal read, naming why and from where.
    pub fn audited(reason: SystemReadReason, origin: impl Into<String>) -> Self {
        Self {
            reason,
            origin: origin.into(),
        }
    }

    /// Why this read is unfiltered.
    pub fn reason(&self) -> SystemReadReason {
        self.reason
    }

    /// Where it came from.
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

/// A context that may serve results **to a client**.
///
/// Implemented for [`AuthorizedReadContext`] and nothing else. Client-facing
/// sinks (query results, CDC sinks fed by a client subscription, Flight streams)
/// take `impl ClientServable`; internal-only sinks take whichever context they
/// like. This is TF-2 S5's "a `SystemReadContext` must never feed a client
/// sink", expressed as a trait bound instead of a code-review rule.
///
/// ```compile_fail
/// use proximadb_abac::{ClientServable, SystemReadContext, SystemReadReason};
///
/// fn serve_to_client(_ctx: &impl ClientServable) {}
///
/// // A SystemReadContext is unfiltered; it must not reach a client sink.
/// let sys = SystemReadContext::audited(SystemReadReason::Compaction, "compactor");
/// serve_to_client(&sys);
/// ```
pub trait ClientServable {
    /// The subject the results will be served to.
    fn subject(&self) -> &SubjectAttributes;
    /// The cache key component results must be stored under.
    fn subject_cache_key(&self) -> SubjectCacheKey;
}

impl ClientServable for AuthorizedReadContext {
    fn subject(&self) -> &SubjectAttributes {
        AuthorizedReadContext::subject(self)
    }

    fn subject_cache_key(&self) -> SubjectCacheKey {
        AuthorizedReadContext::subject_cache_key(self)
    }
}

#[cfg(test)]
mod tests {
    use proximadb_catalog::fc_metamodel::AttrValue;

    use super::*;
    use crate::authority::{AttributeBinding, InMemoryAttributeAuthority};
    use crate::cache_key::InMemoryPolicyEpochs;

    const TENANT: TenantStableId = 7;
    const NS: NamespaceId = 3;
    const TABLE: u32 = 200;

    fn authority() -> InMemoryAttributeAuthority {
        let mut a = InMemoryAttributeAuthority::new();
        a.upsert(
            AttributeBinding::new("alice", TENANT).with_attr("dept", AttrValue::Str("eng".into())),
        );
        a.upsert(
            AttributeBinding::new("bob", TENANT).with_attr("dept", AttrValue::Str("hr".into())),
        );
        a
    }

    fn target() -> Target {
        Target {
            namespace: NS,
            table: TABLE,
            column: None,
        }
    }

    fn binding(object_id: ObjectId, scope: Scope, effect: Effect) -> PolicyBinding {
        PolicyBinding {
            object_id,
            tenant_stable_id: TENANT,
            scope,
            effect,
            predicate_ref: None,
            field_mask: None,
        }
    }

    fn decide(bindings: &[PolicyBinding], who: &str) -> ReadDecision {
        AuthorizedReadContext::resolve(
            &authority(),
            &InMemoryPolicyEpochs::new(),
            bindings,
            &SubjectId(who.into()),
            TENANT,
            target(),
        )
    }

    #[test]
    fn a_table_permit_admits() {
        let d = decide(&[binding(1, Scope::Table(TABLE), Effect::Permit)], "alice");
        let ctx = d.admitted().expect("permitted");
        assert_eq!(ctx.target().table, TABLE);
        assert_eq!(
            ctx.subject().load_bearing("dept"),
            Some(&AttrValue::Str("eng".into()))
        );
    }

    #[test]
    fn no_binding_denies_rather_than_returning_an_unfiltered_context() {
        // The fail-closed default: an unconfigured container is closed.
        assert_eq!(
            decide(&[], "alice"),
            ReadDecision::Deny(DenyReason::NoApplicablePolicy)
        );
    }

    #[test]
    fn a_namespace_deny_masks_a_table_permit() {
        let d = decide(
            &[
                binding(1, Scope::Table(TABLE), Effect::Permit),
                binding(2, Scope::Namespace(NS), Effect::Deny),
            ],
            "alice",
        );
        assert_eq!(d, ReadDecision::Deny(DenyReason::ExplicitDeny));
    }

    #[test]
    fn an_unresolvable_subject_denies_before_policy_is_consulted() {
        // Even with a blanket Permit, an unresolved subject reads nothing — a
        // policy evaluated against unresolved attributes is not evaluated.
        let d = decide(
            &[binding(1, Scope::Table(TABLE), Effect::Permit)],
            "mallory",
        );
        assert!(matches!(
            d,
            ReadDecision::Deny(DenyReason::AttributeResolution(
                AuthorityError::NoBinding { .. }
            ))
        ));
    }

    #[test]
    fn an_unavailable_authority_denies_even_under_a_permit() {
        let mut a = authority();
        a.set_unavailable("store timeout");
        let d = AuthorizedReadContext::resolve(
            &a,
            &InMemoryPolicyEpochs::new(),
            &[binding(1, Scope::Table(TABLE), Effect::Permit)],
            &SubjectId("alice".into()),
            TENANT,
            target(),
        );
        assert!(matches!(
            d,
            ReadDecision::Deny(DenyReason::AttributeResolution(
                AuthorityError::Unavailable { .. }
            ))
        ));
    }

    #[test]
    fn another_tenants_binding_cannot_authorize_this_read() {
        // A Permit issued by tenant 8 must not admit a tenant-7 read; it is
        // filtered out, so the read falls through to the fail-closed default.
        let mut foreign = binding(1, Scope::Table(TABLE), Effect::Permit);
        foreign.tenant_stable_id = TENANT + 1;
        assert_eq!(
            decide(&[foreign], "alice"),
            ReadDecision::Deny(DenyReason::NoApplicablePolicy)
        );
    }

    #[test]
    fn a_foreign_tenants_deny_cannot_veto_this_read_either() {
        // Isolation cuts both ways — tenant 8 must not be able to deny tenant 7.
        let mut foreign_deny = binding(2, Scope::Namespace(NS), Effect::Deny);
        foreign_deny.tenant_stable_id = TENANT + 1;
        let d = decide(
            &[
                binding(1, Scope::Table(TABLE), Effect::Permit),
                foreign_deny,
            ],
            "alice",
        );
        assert!(d.admitted().is_some());
    }

    #[test]
    fn row_predicates_from_applicable_permits_ride_the_context() {
        let mut permit = binding(1, Scope::Table(TABLE), Effect::Permit);
        permit.predicate_ref = Some(555);
        let ctx = decide(&[permit], "alice").admitted().expect("permitted");
        assert_eq!(ctx.row_predicate_refs(), &[555]);
    }

    #[test]
    fn field_masks_apply_per_column_and_default_to_visible() {
        let mut mask = binding(
            2,
            Scope::Column {
                table: TABLE,
                column: 4,
            },
            Effect::Permit,
        );
        mask.field_mask = Some(FieldMask::Redact);
        let ctx = decide(
            &[binding(1, Scope::Table(TABLE), Effect::Permit), mask],
            "alice",
        )
        .admitted()
        .expect("permitted");

        assert_eq!(ctx.column_decision(4), ColumnDecision::Redact);
        assert_eq!(ctx.column_decision(0), ColumnDecision::Visible);
        assert_eq!(
            ctx.masked_columns().collect::<Vec<_>>(),
            vec![(4, ColumnDecision::Redact)]
        );
    }

    #[test]
    fn the_strictest_mask_wins_when_two_bindings_cover_one_column() {
        let col = Scope::Column {
            table: TABLE,
            column: 4,
        };
        let mut lenient = binding(2, col.clone(), Effect::Permit);
        lenient.field_mask = Some(FieldMask::Null);
        let mut strict = binding(3, col, Effect::Permit);
        strict.field_mask = Some(FieldMask::Forbid);

        let ctx = decide(
            &[
                binding(1, Scope::Table(TABLE), Effect::Permit),
                lenient,
                strict,
            ],
            "alice",
        )
        .admitted()
        .expect("permitted");
        assert_eq!(ctx.column_decision(4), ColumnDecision::Forbidden);
    }

    #[test]
    fn a_column_scoped_deny_forbids_that_column_even_without_a_mask() {
        let deny_col = binding(
            2,
            Scope::Column {
                table: TABLE,
                column: 4,
            },
            Effect::Deny,
        );
        let ctx = decide(
            &[binding(1, Scope::Table(TABLE), Effect::Permit), deny_col],
            "alice",
        )
        .admitted()
        .expect("row read is still permitted");
        assert_eq!(ctx.column_decision(4), ColumnDecision::Forbidden);
    }

    #[test]
    fn a_forbidden_column_rejects_the_query_rather_than_nulling_itself() {
        let mut forbid = binding(
            2,
            Scope::Column {
                table: TABLE,
                column: 4,
            },
            Effect::Permit,
        );
        forbid.field_mask = Some(FieldMask::Forbid);
        let ctx = decide(
            &[binding(1, Scope::Table(TABLE), Effect::Permit), forbid],
            "alice",
        )
        .admitted()
        .expect("permitted");

        assert_eq!(ctx.project(&[0, 1]).expect("visible columns").len(), 2);
        assert_eq!(
            ctx.project(&[0, 4]),
            Err(ForbiddenColumn { ordinal: 4 }),
            "forbid is query-rejection, not a null projection"
        );
    }

    #[test]
    fn two_subjects_with_different_attributes_get_different_cache_keys() {
        let permits = [binding(1, Scope::Table(TABLE), Effect::Permit)];
        let alice = decide(&permits, "alice").admitted().expect("permitted");
        let bob = decide(&permits, "bob").admitted().expect("permitted");
        assert_ne!(alice.subject_cache_key(), bob.subject_cache_key());
    }

    #[test]
    fn the_cache_key_tracks_the_policy_epoch() {
        let permits = [binding(1, Scope::Table(TABLE), Effect::Permit)];
        let a = authority();
        let mut epochs = InMemoryPolicyEpochs::new();

        let before = AuthorizedReadContext::resolve(
            &a,
            &epochs,
            &permits,
            &SubjectId("alice".into()),
            TENANT,
            target(),
        )
        .admitted()
        .expect("permitted")
        .subject_cache_key();

        epochs.bump(TENANT, NS);

        let after = AuthorizedReadContext::resolve(
            &a,
            &epochs,
            &permits,
            &SubjectId("alice".into()),
            TENANT,
            target(),
        )
        .admitted()
        .expect("permitted")
        .subject_cache_key();

        assert_ne!(before, after);
    }

    #[test]
    fn a_client_sink_accepts_an_authorized_context() {
        // The positive half of the compile_fail doctest on ClientServable: an
        // AuthorizedReadContext is exactly what a client sink takes.
        fn serve(ctx: &impl ClientServable) -> SubjectCacheKey {
            ctx.subject_cache_key()
        }
        let ctx = decide(&[binding(1, Scope::Table(TABLE), Effect::Permit)], "alice")
            .admitted()
            .expect("permitted");
        assert_eq!(serve(&ctx), ctx.subject_cache_key());
    }

    #[test]
    fn a_system_read_context_names_its_reason() {
        let sys = SystemReadContext::audited(SystemReadReason::Compaction, "flush_materializer");
        assert_eq!(sys.reason(), SystemReadReason::Compaction);
        assert_eq!(sys.origin(), "flush_materializer");
        // It has no subject and no cache key by construction — there is nothing
        // to serve a client with.
    }

    #[test]
    fn a_denied_decision_yields_no_context_to_unwrap() {
        let d = decide(&[], "alice");
        assert!(d.is_deny());
        assert!(d.admitted().is_none());
    }
}

// ===========================================================================
// ReadContext — the unified required parameter (Phase 4 / FA-b)
// ===========================================================================

/// The **required** read context every client-servicing read primitive takes
/// (Phase 4 / FA-b). Unifies the two access modes into one non-`Option` type,
/// so a read that bypasses authorization does not compile.
///
/// - `Client` — a resolved [`AuthorizedReadContext`]; the scan applies the
///   subject's ABAC policy. Required for every client-servicing read.
/// - `System` — an audited [`SystemReadContext`]; the scan runs unfiltered.
///   Required for internal reads (compaction, FK resolution, index build,
///   replication, recovery).
///
/// Construct `System` via [`ReadContext::system`]; construct `Client` by
/// resolving an [`AuthorizedReadContext`] and wrapping it. There is no
/// `Default` and no `unfiltered()` — the type has no "I forgot" escape.
#[derive(Debug, Clone)]
pub enum ReadContext {
    /// A client-servicing read — the subject's ABAC policy applies.
    Client(AuthorizedReadContext),
    /// An internal read — unfiltered, but audited (names the reason).
    System(SystemReadContext),
}

impl ReadContext {
    /// Shortcut for an audited system read.
    pub fn system(reason: SystemReadReason, origin: impl Into<String>) -> Self {
        ReadContext::System(SystemReadContext::audited(reason, origin))
    }

    /// Whether this is a client-servicing context (ABAC applies).
    pub fn is_client(&self) -> bool {
        matches!(self, ReadContext::Client(_))
    }

    /// The [`AuthorizedReadContext`], if this is a client read.
    pub fn as_client(&self) -> Option<&AuthorizedReadContext> {
        match self {
            ReadContext::Client(ctx) => Some(ctx),
            ReadContext::System(_) => None,
        }
    }
}

#[cfg(test)]
mod read_context_tests {
    use super::*;

    #[test]
    fn system_context_is_not_client() {
        let ctx = ReadContext::system(SystemReadReason::Compaction, "compactor");
        assert!(!ctx.is_client());
        assert!(ctx.as_client().is_none());
    }
}
