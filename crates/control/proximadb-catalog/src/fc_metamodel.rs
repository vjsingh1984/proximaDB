// Copyright (C) 2026 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! # FC metamodel — first-class relationships + the family-parameterized identity slot
//!
//! Implements the **D12 metamodel** tranche of TD-FOUNDATION-2 (ADR-072 D12):
//! graph edges / FKs / references become a first-class [`RelationshipType`]
//! (peer of the entity, not a value in the identity slot), and identity becomes
//! a **set of facets over two logical axes** ([`IdentitySlot`]).
//!
//! ## Scope (why this is additive + inert)
//!
//! This module is **data structures + validation only** — it defines the
//! metamodel and the policy *model*; it performs **no enforcement**. Enforcement
//! (the always-ANDed ABAC runtime filter) is the separate FA track
//! (TD-FOUNDATION-3), which is blocked on its own preconditions. Everything here
//! is behind the `fc-metamodel` feature and wired into nothing yet.
//!
//! ## Identity authority (ADR-075, coordinates with #1223)
//!
//! Every id-reference keys on the **stable numeric ids** — catalog
//! `object_id: u64`, `tenant_stable_id: u64`, `NamespaceId: u16`,
//! `CollectionId: u32`. Display strings are a derived legacy-compat form resolved
//! at the ADR-074 identity seam; they are **never** an identity or a policy scope
//! here. (The `internal::CatalogObject` String-UUID `object_id` is the
//! non-authoritative Model-B id; reconciling it is the parallel-model collapse,
//! out of FC scope.)

use serde::{Deserialize, Serialize};

/// The authoritative catalog object id (ADR-031 / ADR-075). Stable, immutable,
/// rename-safe, never reused.
pub type ObjectId = u64;
/// Stable tenant id (ADR-075). Authoritative over the display `tenant_id` string.
pub type TenantStableId = u64;
/// Stable namespace id (ADR-075; mirrors `proximadb_kernel::stable_id::NamespaceId`).
pub type NamespaceId = u16;
/// Stable collection/table id (ADR-075; mirrors `proximadb_kernel::stable_id::CollectionId`).
pub type CollectionId = u32;

/// The modality family an [`EntityRef`] endpoint or an identity slot belongs to.
/// The identity slot is **family-parameterized** (D12-b): different families
/// carry different identity facets (relational has a PK; graph nodes omit it;
/// key-value detaches it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityFamily {
    /// Relational row / RDBMS table.
    Relational,
    /// Document (JSON/BSON) record.
    Document,
    /// Graph node.
    GraphNode,
    /// Vector / embedding record.
    Vector,
    /// Time-series sample.
    TimeSeries,
    /// Key-value pair.
    KeyValue,
}

/// A typed, **stable-id** reference to a catalog entity — a `RelationshipType`
/// endpoint. Keyed on the authoritative `object_id` (never a display string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    /// The referenced catalog object (ADR-075 stable id).
    pub object_id: ObjectId,
    /// Which family the endpoint is, so a relationship can span models
    /// (relational FK ↔ graph node ↔ document ↔ vector).
    pub family: EntityFamily,
}

/// Edge direction for a [`RelationshipType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    /// Directed edge (`from → to`).
    Directed,
    /// Undirected edge.
    Undirected,
}

/// Cardinality of a [`RelationshipType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Multiplicity {
    /// One-to-one.
    OneToOne,
    /// One-to-many.
    OneToMany,
    /// Many-to-many.
    ManyToMany,
}

/// Cross-tenant policy for a relationship (ADR-072 D12-d). FC ships **`Intra`
/// only**; `Reference`/`Brokered` are DEFERRED (they need a real graph executor +
/// attribute federation — see TD-FOUNDATION-2 §2 D12-d).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationshipTenancy {
    /// Both endpoints resolve to the same tenant (fail-closed default).
    Intra,
}

/// A **first-class relationship** (D12-a): a peer of the entity, with its own
/// stable identity and attributes — not a value smuggled into the identity slot.
/// A relational FK and a graph edge are the *same* metamodel object with
/// different physical projections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationshipType {
    /// Stable catalog id (ADR-075).
    pub object_id: ObjectId,
    /// Owning tenant (stable id).
    pub tenant_stable_id: TenantStableId,
    /// The `from` endpoint.
    pub from: EntityRef,
    /// The `to` endpoint.
    pub to: EntityRef,
    /// Directed or undirected.
    pub direction: Direction,
    /// Cardinality.
    pub multiplicity: Multiplicity,
    /// Edges carry their own properties — logical column names (the physical
    /// layout lives in `CatalogStorageLayout`, kept separate per D12-c).
    pub attributes: Vec<String>,
    /// Cross-tenant policy (FC: `Intra` only).
    pub tenancy: RelationshipTenancy,
}

impl RelationshipType {
    /// `Intra` invariant (D12-d, FC scope): both endpoints must be same-tenant.
    /// The endpoint tenants are validated by the catalog at write time; this
    /// checks the tenancy tag is one FC admits.
    pub fn tenancy_is_admissible(&self) -> bool {
        matches!(self.tenancy, RelationshipTenancy::Intra)
    }
}

/// A logical reference to a column that participates in a key or a policy — by
/// stable ordinal within its owning object, plus its name for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ColumnRef {
    /// Stable column ordinal within the owning object.
    pub ordinal: u32,
    /// Column name (diagnostics / display; never an identity).
    pub name: String,
}

/// The **logical** spec of a composite key — *which* columns form it, in order.
/// The runtime bytes are produced by the F0 codec
/// (`proximadb_data_model::key_codec::encode_identity`) over the row's values;
/// this metamodel type says only what the key *is*, not its encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositeKeySpec {
    /// The keyspace (logical table/collection name component of the F0 key).
    pub keyspace: String,
    /// The ordered columns forming the key.
    pub columns: Vec<ColumnRef>,
}

/// Axis 1 — the **identity role**: what the key guarantees. (Axis 2, the physical
/// access path, is deliberately **not** here — it is a `CatalogStorageLayout` /
/// index concern per D12-c, and putting it in the logical identity slot was the
/// round-2 red-team break.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityRole {
    /// The natural primary key. **At most one per slot**; `oid == key` (matches
    /// the CKS `Oid` contract). Drives the fenced CKS registration.
    UniquenessPrimary,
    /// A secondary `UNIQUE`. Zero-or-more per slot; the CKS registers
    /// `oid == owning row's primary key`, referenced here.
    UniquenessSecondary {
        /// The primary key this UNIQUE's owning row is identified by.
        owning_primary: CompositeKeySpec,
    },
    /// Order-preserving, **not** uniqueness-fenced (time-series). `axis` names
    /// which ordering (e.g. event-time vs ingest-time) it binds.
    Ordering {
        /// Which ordering axis this facet binds.
        axis: String,
    },
    /// A row surrogate (e.g. a vector row-id): neither uniqueness-fenced nor
    /// ordered on its own.
    Surrogate,
}

/// One identity facet (D12-b). A record may have several (a multi-vector row is
/// one `UniquenessPrimary` + N `Surrogate`; a time-series sample is a
/// `UniquenessPrimary` over `(series, ts)` + an `Ordering{ts}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityFacet {
    /// The logical role (axis 1).
    pub role: IdentityRole,
    /// The composite key this facet is over (F0-encoded at runtime;
    /// CKS-fenced iff the role is a `Uniqueness*`).
    pub key: CompositeKeySpec,
}

impl IdentityFacet {
    /// Whether this facet is registered in the ConditionalKeyStore (F1a/F2) —
    /// true for any uniqueness role.
    pub fn is_cks_fenced(&self) -> bool {
        matches!(
            self.role,
            IdentityRole::UniquenessPrimary | IdentityRole::UniquenessSecondary { .. }
        )
    }
}

/// A record's identity — a **set of facets** over the two-axis model (D12-b),
/// plus the explicit `filterable_columns` list the ABAC planner consumes (this,
/// not the access path, is the honest signal that a collection carries filterable
/// columns — the round-2 fix).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitySlot {
    /// The identity facets (>1 for multi-vector rows and TS composites).
    pub facets: Vec<IdentityFacet>,
    /// The attribute columns an ABAC row/field policy may reference (FA/D15).
    pub filterable_columns: Vec<ColumnRef>,
    /// U-Schema discipline: what this family's identity omits.
    pub completeness_gaps: Vec<String>,
}

/// Error validating an [`IdentitySlot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// More than one `UniquenessPrimary` facet (F2's single-PK invariant).
    MultiplePrimaryFacets(usize),
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdentityError::MultiplePrimaryFacets(n) => write!(
                f,
                "identity slot has {n} UniquenessPrimary facets; at most one is allowed (F2 single-PK invariant)"
            ),
        }
    }
}
impl std::error::Error for IdentityError {}

impl IdentitySlot {
    /// The number of `UniquenessPrimary` facets.
    pub fn primary_count(&self) -> usize {
        self.facets
            .iter()
            .filter(|f| matches!(f.role, IdentityRole::UniquenessPrimary))
            .count()
    }

    /// Enforce the metamodel invariant: **at most one** `UniquenessPrimary`
    /// (restores F2's single natural-PK contract; a `Vec<facet>` alone would
    /// silently allow two — the round-2 red-team break).
    pub fn validate(&self) -> Result<(), IdentityError> {
        let n = self.primary_count();
        if n > 1 {
            return Err(IdentityError::MultiplePrimaryFacets(n));
        }
        Ok(())
    }
}

/// The container in the governance hierarchy a [`PolicyBinding`] applies to —
/// keyed on stable ids (D12 / ADR-075). Never a display string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// The whole namespace/database.
    Namespace(NamespaceId),
    /// A collection/table.
    Table(CollectionId),
    /// A single column/field (owning object + column ordinal).
    Column {
        /// Owning table.
        table: CollectionId,
        /// Column ordinal.
        column: u32,
    },
}

/// A policy **binding** object (the policy *model* — FC delivers the data
/// structure; FA does the enforcement). Lives as a catalog object in the reserved
/// system namespace. `Deny` wins; the default is deny (fail-closed) — resolved by
/// the (FA-track) enforcement, not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyBinding {
    /// Stable catalog id of this binding.
    pub object_id: ObjectId,
    /// The tenant whose authority owns this binding (ADR-075 stable id).
    pub tenant_stable_id: TenantStableId,
    /// The container this binding governs.
    pub scope: Scope,
    /// Permit or deny (deny wins during resolution).
    pub effect: Effect,
    /// The opaque predicate id resolved by FA against the subject's attributes.
    /// FC carries the reference; FA compiles/evaluates it.
    pub predicate_ref: Option<ObjectId>,
    /// Optional column-level mask.
    pub field_mask: Option<FieldMask>,
}

/// Permit vs deny (deny wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    /// Grant (subject to composition + any deny at a broader scope).
    Permit,
    /// Deny (wins over any permit).
    Deny,
}

/// Column-level masking. `Forbid` is query-rejection (not a null projection —
/// the round-2 fix); `Redact`/`Null` are projection rewrites (applied by FA).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FieldMask {
    /// Null the column out on read.
    Null,
    /// Redact (mask) the value on read.
    Redact,
    /// Forbid — the read is rejected (existence/schema must not leak).
    Forbid,
}

// ===========================================================================
// Derivation from the catalog schema (attaches the metamodel to Model A)
// ===========================================================================

use crate::{CatalogColumn, CatalogTableSchema};

/// Whether a column's logical type is a vector modality — such a column gets a
/// `Surrogate` facet (its similarity access path is physical, not in the slot).
fn is_vector_column(col: &CatalogColumn) -> bool {
    use proximadb_data_model::ProximaType;
    matches!(
        col.data_type,
        ProximaType::DenseVector { .. }
            | ProximaType::SparseVector { .. }
            | ProximaType::BinaryVector { .. }
    )
}

/// Resolve a column name to a [`ColumnRef`] against the schema, using the stable
/// physical field id (`CatalogColumn::id`) as the ordinal; falls back to the
/// positional index for a name not found in the schema.
fn column_ref(schema: &CatalogTableSchema, name: &str) -> ColumnRef {
    match schema
        .columns
        .iter()
        .enumerate()
        .find(|(_, c)| c.name == name)
    {
        Some((_, c)) => ColumnRef {
            ordinal: c.id.max(0) as u32,
            name: name.to_string(),
        },
        None => ColumnRef {
            ordinal: schema.columns.len() as u32,
            name: name.to_string(),
        },
    }
}

/// Derive the [`IdentitySlot`] for a table from its catalog schema (D12-b).
///
/// This is the seam by which F2's CKS registration reads its natural key **from
/// the metamodel**: the `UniquenessPrimary` facet's `key` is exactly the PK the
/// ConditionalKeyStore fences. Derivation rules:
///
/// - `primary_key` (non-empty) → one `UniquenessPrimary` facet.
/// - each **unique index** whose columns differ from the PK → a
///   `UniquenessSecondary` facet referencing the PK as `owning_primary`
///   (skipped, with a gap noted, when there is no PK to own it).
/// - each **vector column** → a `Surrogate` facet.
/// - non-vector, non-deleted columns → `filterable_columns` (the ABAC allow-list
///   source — the round-2 fix; this, not an access path, is the honest signal).
/// - no PK → a `completeness_gaps` note.
///
/// Physical access path is deliberately **not** derived (D12-c: it lives in the
/// storage layout / index set, never the logical identity slot).
pub fn identity_slot_from_table_schema(schema: &CatalogTableSchema) -> IdentitySlot {
    let keyspace = schema.name.clone();
    let mut facets = Vec::new();
    let mut gaps = Vec::new();

    // 1. Primary key → UniquenessPrimary (the CKS-fenced natural key).
    let primary_spec = if schema.primary_key.is_empty() {
        gaps.push("no natural primary key declared".to_string());
        None
    } else {
        let spec = CompositeKeySpec {
            keyspace: keyspace.clone(),
            columns: schema
                .primary_key
                .iter()
                .map(|n| column_ref(schema, n))
                .collect(),
        };
        facets.push(IdentityFacet {
            role: IdentityRole::UniquenessPrimary,
            key: spec.clone(),
        });
        Some(spec)
    };

    // 2. Unique indexes → UniquenessSecondary (only when a PK owns the row).
    for idx in &schema.indexes {
        if !idx.is_unique || idx.columns == schema.primary_key {
            continue;
        }
        let key = CompositeKeySpec {
            keyspace: keyspace.clone(),
            columns: idx.columns.iter().map(|n| column_ref(schema, n)).collect(),
        };
        match &primary_spec {
            Some(pk) => facets.push(IdentityFacet {
                role: IdentityRole::UniquenessSecondary {
                    owning_primary: pk.clone(),
                },
                key,
            }),
            None => gaps.push(format!(
                "unique index '{}' has no primary key to own it",
                idx.name
            )),
        }
    }

    // 3. Vector columns → Surrogate facets.
    for col in &schema.columns {
        if !col.is_deleted && is_vector_column(col) {
            facets.push(IdentityFacet {
                role: IdentityRole::Surrogate,
                key: CompositeKeySpec {
                    keyspace: keyspace.clone(),
                    columns: vec![column_ref(schema, &col.name)],
                },
            });
        }
    }

    // 4. filterable_columns = non-vector, non-deleted columns (ABAC allow-list).
    let filterable_columns = schema
        .columns
        .iter()
        .filter(|c| !c.is_deleted && !is_vector_column(c))
        .map(|c| ColumnRef {
            ordinal: c.id.max(0) as u32,
            name: c.name.clone(),
        })
        .collect();

    IdentitySlot {
        facets,
        filterable_columns,
        completeness_gaps: gaps,
    }
}

// ===========================================================================
// Hierarchical policy resolver (the policy *model* — inert, no enforcement)
// ===========================================================================

/// The container a read targets, addressed by stable ids (ADR-075). A policy
/// binding at a broader scope (namespace ⊇ table ⊇ column) applies to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Target {
    /// The namespace the target lives in.
    pub namespace: NamespaceId,
    /// The table/collection.
    pub table: CollectionId,
    /// A specific column, when resolving column scope (else `None`).
    pub column: Option<u32>,
}

/// The composed policy for a [`Target`] — the **model** output. It records the
/// effect-level decision (assuming every predicate holds), the row-predicate refs
/// to AND, and the field masks. It does **not** evaluate predicates against a
/// subject — that is FA's enforcement job (TD-FOUNDATION-3). Default is
/// [`Effect::Deny`] (fail-closed) when no binding applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectivePolicy {
    /// The effect-level decision after deny-bias composition (a `Deny` binding
    /// at any applicable scope wins). `Deny` with `applicable == 0` is the
    /// fail-closed default (no binding).
    pub decision: Effect,
    /// Whether any binding actually applied (distinguishes an explicit `Deny`
    /// from the fail-closed default).
    pub applicable: usize,
    /// Row-predicate object refs to AND together (from applicable `Permit`
    /// bindings). Empty = no row restriction *at the model level*; FA still
    /// resolves them against the subject.
    pub predicate_refs: Vec<ObjectId>,
    /// Field masks to apply, keyed by column ordinal (from column-scoped bindings).
    pub field_masks: Vec<(u32, FieldMask)>,
}

/// Does a binding's [`Scope`] cover the [`Target`] (broader-or-equal in the
/// namespace ⊇ table ⊇ column hierarchy)?
fn scope_covers(scope: &Scope, target: &Target) -> bool {
    match *scope {
        Scope::Namespace(ns) => ns == target.namespace,
        Scope::Table(t) => t == target.table,
        Scope::Column { table, column } => table == target.table && target.column == Some(column),
    }
}

/// Resolve the effective policy for a `target` from the given bindings (TF-2
/// §3.2). **Deny-biased and hierarchical**: any applicable `Deny` wins; otherwise
/// the applicable `Permit`s contribute their row-predicate refs (ANDed) and any
/// column-scoped bindings contribute field masks. Fail-closed: no applicable
/// binding ⇒ `Deny` with `applicable == 0`.
///
/// This is the policy **model** — it composes structure, not a subject decision.
/// FA (TD-FOUNDATION-3) evaluates `predicate_refs` against the subject to admit or
/// deny individual rows.
pub fn resolve_effective_policy(bindings: &[PolicyBinding], target: &Target) -> EffectivePolicy {
    let mut applicable = 0usize;
    let mut has_deny = false;
    let mut predicate_refs = Vec::new();
    let mut field_masks = Vec::new();

    for b in bindings {
        if !scope_covers(&b.scope, target) {
            continue;
        }
        applicable += 1;
        match b.effect {
            Effect::Deny => has_deny = true,
            Effect::Permit => {
                if let Some(p) = b.predicate_ref {
                    predicate_refs.push(p);
                }
            }
        }
        if let (Scope::Column { column, .. }, Some(mask)) = (&b.scope, b.field_mask) {
            field_masks.push((*column, mask));
        }
    }

    let decision = if applicable == 0 || has_deny {
        Effect::Deny // fail-closed default, or an explicit deny wins
    } else {
        Effect::Permit
    };

    EffectivePolicy {
        decision,
        applicable,
        predicate_refs,
        field_masks,
    }
}

// ===========================================================================
// Subject attributes (ABAC's "A") — with the attribute-trust boundary as a type
// ===========================================================================

use std::collections::BTreeMap;

/// An opaque authenticated principal id, resolved at the identity seam (ADR-074).
/// A subject is a *principal*, not a catalog object — hence a string, not a
/// stable catalog id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubjectId(pub String);

/// An ABAC attribute value. `Int` supports ordered comparisons (e.g. clearance
/// levels); `List` supports membership (e.g. group/role sets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttrValue {
    /// A string attribute (dept, region, …).
    Str(String),
    /// An integer attribute (clearance level, …).
    Int(i64),
    /// A boolean attribute.
    Bool(bool),
    /// A list attribute (groups, roles, …).
    List(Vec<String>),
}

/// Where an attribute value came from — **the attribute-trust boundary as a
/// type** (round-2/3 red-team fix). Only [`AttrSource::ServerResolved`] values
/// may be *load-bearing* in an authz predicate; [`AttrSource::Claim`] values are
/// advisory labels a tenant-controlled IdP asserted and must **never** decide an
/// admit/deny (else a forged `clearance`/`role` claim escalates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttrSource {
    /// Asserted by the subject's IdP claims — advisory only, not load-bearing.
    Claim,
    /// Re-resolved server-side against the policy's tenant authority — load-bearing.
    ServerResolved,
}

/// One attribute: its value plus its provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attr {
    /// The value.
    pub value: AttrValue,
    /// Where it came from (governs whether it is load-bearing).
    pub source: AttrSource,
}

/// The subject's attribute bag (ABAC's "A"). Carries dept/clearance/region/etc.,
/// each tagged with its [`AttrSource`]. **Privilege-granting roles are not a
/// special field** — a role is just an attribute, and like every attribute it is
/// load-bearing only when `ServerResolved`. This is how the model closes
/// admin-by-claims and the `Reference` forged-attribute leak: FA reads attributes
/// via [`SubjectAttributes::load_bearing`], which returns `None` for a claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectAttributes {
    /// The authenticated principal.
    pub subject_id: SubjectId,
    /// The tenant the subject is scoped to (ADR-075 stable id, from the seam).
    pub tenant_stable_id: TenantStableId,
    attrs: BTreeMap<String, Attr>,
}

impl SubjectAttributes {
    /// A subject with no attributes yet.
    pub fn new(subject_id: impl Into<String>, tenant_stable_id: TenantStableId) -> Self {
        Self {
            subject_id: SubjectId(subject_id.into()),
            tenant_stable_id,
            attrs: BTreeMap::new(),
        }
    }

    /// Add a **claim-sourced** attribute (advisory; never load-bearing). Use this
    /// for values that arrive in an SSO/OIDC token.
    pub fn with_claim(mut self, key: impl Into<String>, value: AttrValue) -> Self {
        self.attrs.insert(
            key.into(),
            Attr {
                value,
                source: AttrSource::Claim,
            },
        );
        self
    }

    /// Add a **server-resolved** attribute (authoritative; load-bearing). Use this
    /// for values `resolve_effective_attributes` produced against the tenant's
    /// authority.
    pub fn with_resolved(mut self, key: impl Into<String>, value: AttrValue) -> Self {
        self.attrs.insert(
            key.into(),
            Attr {
                value,
                source: AttrSource::ServerResolved,
            },
        );
        self
    }

    /// The attribute (value + provenance) for `key`, regardless of source —
    /// suitable for display/audit, **not** for an authz decision.
    pub fn get(&self, key: &str) -> Option<&Attr> {
        self.attrs.get(key)
    }

    /// The **load-bearing** value for `key` — `Some` only when the attribute is
    /// `ServerResolved`. This is the accessor FA uses inside a predicate; a
    /// claim-sourced value returns `None`, so a forged claim cannot satisfy a
    /// policy (the attribute-trust boundary, enforced by the type).
    pub fn load_bearing(&self, key: &str) -> Option<&AttrValue> {
        match self.attrs.get(key) {
            Some(Attr {
                value,
                source: AttrSource::ServerResolved,
            }) => Some(value),
            _ => None,
        }
    }

    /// The number of attributes carried (any source).
    pub fn len(&self) -> usize {
        self.attrs.len()
    }

    /// Whether the bag is empty.
    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(keyspace: &str, col: &str) -> CompositeKeySpec {
        CompositeKeySpec {
            keyspace: keyspace.into(),
            columns: vec![ColumnRef {
                ordinal: 0,
                name: col.into(),
            }],
        }
    }

    #[test]
    fn relationship_is_first_class_and_intra_only() {
        let r = RelationshipType {
            object_id: 42,
            tenant_stable_id: 7,
            from: EntityRef {
                object_id: 100,
                family: EntityFamily::Relational,
            },
            to: EntityRef {
                object_id: 200,
                family: EntityFamily::GraphNode,
            },
            direction: Direction::Directed,
            multiplicity: Multiplicity::ManyToMany,
            attributes: vec!["weight".into()],
            tenancy: RelationshipTenancy::Intra,
        };
        // A relational endpoint and a graph endpoint in one relationship: the D1
        // unification. And FC only admits Intra.
        assert!(r.tenancy_is_admissible());
        assert_eq!(r.from.family, EntityFamily::Relational);
        assert_eq!(r.to.family, EntityFamily::GraphNode);
    }

    #[test]
    fn multi_vector_row_has_one_primary_plus_n_surrogates() {
        let slot = IdentitySlot {
            facets: vec![
                IdentityFacet {
                    role: IdentityRole::UniquenessPrimary,
                    key: pk("users", "id"),
                },
                IdentityFacet {
                    role: IdentityRole::Surrogate,
                    key: pk("users", "title_vec"),
                },
                IdentityFacet {
                    role: IdentityRole::Surrogate,
                    key: pk("users", "body_vec"),
                },
            ],
            filterable_columns: vec![ColumnRef {
                ordinal: 3,
                name: "dept".into(),
            }],
            completeness_gaps: vec![],
        };
        assert_eq!(slot.primary_count(), 1);
        assert!(slot.validate().is_ok());
        // The PK is CKS-fenced; the vector surrogates are not.
        assert!(slot.facets[0].is_cks_fenced());
        assert!(!slot.facets[1].is_cks_fenced());
    }

    #[test]
    fn secondary_unique_references_its_owning_primary_and_is_fenced() {
        let facet = IdentityFacet {
            role: IdentityRole::UniquenessSecondary {
                owning_primary: pk("users", "id"),
            },
            key: pk("users", "email"),
        };
        // A secondary UNIQUE is still CKS-fenced, but points at the owning row's PK.
        assert!(facet.is_cks_fenced());
        match &facet.role {
            IdentityRole::UniquenessSecondary { owning_primary } => {
                assert_eq!(owning_primary.columns[0].name, "id");
            }
            _ => panic!("expected secondary"),
        }
    }

    #[test]
    fn timeseries_has_uniqueness_and_ordering_together() {
        let slot = IdentitySlot {
            facets: vec![
                IdentityFacet {
                    role: IdentityRole::UniquenessPrimary,
                    key: CompositeKeySpec {
                        keyspace: "metrics".into(),
                        columns: vec![
                            ColumnRef {
                                ordinal: 0,
                                name: "series".into(),
                            },
                            ColumnRef {
                                ordinal: 1,
                                name: "ts".into(),
                            },
                        ],
                    },
                },
                IdentityFacet {
                    role: IdentityRole::Ordering { axis: "ts".into() },
                    key: pk("metrics", "ts"),
                },
            ],
            filterable_columns: vec![],
            completeness_gaps: vec![],
        };
        // Both roles co-exist — the round-1 break (single enum) is gone.
        assert!(slot.validate().is_ok());
        assert_eq!(slot.primary_count(), 1);
    }

    #[test]
    fn two_primary_facets_are_rejected() {
        let slot = IdentitySlot {
            facets: vec![
                IdentityFacet {
                    role: IdentityRole::UniquenessPrimary,
                    key: pk("t", "a"),
                },
                IdentityFacet {
                    role: IdentityRole::UniquenessPrimary,
                    key: pk("t", "b"),
                },
            ],
            filterable_columns: vec![],
            completeness_gaps: vec![],
        };
        // Restores F2's single-PK invariant that a bare Vec<facet> would void.
        assert_eq!(
            slot.validate(),
            Err(IdentityError::MultiplePrimaryFacets(2))
        );
    }

    #[test]
    fn policy_binding_scopes_key_on_stable_ids() {
        let b = PolicyBinding {
            object_id: 9,
            tenant_stable_id: 7,
            scope: Scope::Column {
                table: 200,
                column: 3,
            },
            effect: Effect::Deny,
            predicate_ref: None,
            field_mask: Some(FieldMask::Forbid),
        };
        // Scope is a stable-id hierarchy address; no display strings.
        assert!(matches!(
            b.scope,
            Scope::Column {
                table: 200,
                column: 3
            }
        ));
        assert_eq!(b.effect, Effect::Deny);
    }

    // --- derivation from the catalog schema ---

    fn dense_vec(dim: usize) -> proximadb_data_model::ProximaType {
        proximadb_data_model::ProximaType::DenseVector {
            element: proximadb_data_model::VectorElement::Float32,
            dim,
        }
    }

    #[test]
    fn derives_primary_secondary_surrogate_from_schema() {
        // users(id PK, email UNIQUE, dept scalar, body_vec vector)
        let schema = CatalogTableSchema::new("users")
            .with_column(CatalogColumn::new(
                0,
                "id",
                proximadb_data_model::ProximaType::Int64,
            ))
            .with_column(CatalogColumn::new(
                1,
                "email",
                proximadb_data_model::ProximaType::String,
            ))
            .with_column(CatalogColumn::new(
                2,
                "dept",
                proximadb_data_model::ProximaType::String,
            ))
            .with_column(CatalogColumn::new(3, "body_vec", dense_vec(384)))
            .with_primary_key(vec!["id".to_string()])
            .with_index(
                crate::CatalogIndex::new(
                    "uq_email",
                    vec!["email".to_string()],
                    crate::CatalogIndexType::BTree,
                )
                .unique(),
            );

        let slot = identity_slot_from_table_schema(&schema);
        assert!(slot.validate().is_ok());
        assert_eq!(slot.primary_count(), 1);

        // one primary (id), one secondary UNIQUE (email → owns id), one surrogate (body_vec)
        let primary: Vec<_> = slot
            .facets
            .iter()
            .filter(|f| matches!(f.role, IdentityRole::UniquenessPrimary))
            .collect();
        assert_eq!(primary.len(), 1);
        assert_eq!(primary[0].key.columns[0].name, "id");

        let secondary: Vec<_> = slot
            .facets
            .iter()
            .filter(|f| matches!(f.role, IdentityRole::UniquenessSecondary { .. }))
            .collect();
        assert_eq!(secondary.len(), 1);
        assert_eq!(secondary[0].key.columns[0].name, "email");
        match &secondary[0].role {
            IdentityRole::UniquenessSecondary { owning_primary } => {
                assert_eq!(owning_primary.columns[0].name, "id"); // owns the PK
            }
            _ => unreachable!(),
        }

        let surrogates: Vec<_> = slot
            .facets
            .iter()
            .filter(|f| matches!(f.role, IdentityRole::Surrogate))
            .collect();
        assert_eq!(surrogates.len(), 1);
        assert_eq!(surrogates[0].key.columns[0].name, "body_vec");

        // filterable_columns excludes the vector, includes the scalars.
        let filt: Vec<&str> = slot
            .filterable_columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(filt.contains(&"dept"));
        assert!(filt.contains(&"email"));
        assert!(!filt.contains(&"body_vec"));
    }

    #[test]
    fn derives_gap_when_no_primary_key() {
        let schema = CatalogTableSchema::new("events").with_column(CatalogColumn::new(
            0,
            "payload",
            proximadb_data_model::ProximaType::String,
        ));
        let slot = identity_slot_from_table_schema(&schema);
        assert_eq!(slot.primary_count(), 0);
        assert!(
            slot.completeness_gaps
                .iter()
                .any(|g| g.contains("no natural primary key"))
        );
    }

    // --- hierarchical policy resolver ---

    fn binding(object_id: ObjectId, scope: Scope, effect: Effect) -> PolicyBinding {
        PolicyBinding {
            object_id,
            tenant_stable_id: 7,
            scope,
            effect,
            predicate_ref: None,
            field_mask: None,
        }
    }

    fn target(ns: NamespaceId, table: CollectionId, column: Option<u32>) -> Target {
        Target {
            namespace: ns,
            table,
            column,
        }
    }

    #[test]
    fn no_binding_is_fail_closed_deny() {
        let eff = resolve_effective_policy(&[], &target(1, 200, None));
        assert_eq!(eff.decision, Effect::Deny);
        assert_eq!(eff.applicable, 0); // the fail-closed default, not an explicit deny
    }

    #[test]
    fn namespace_deny_masks_a_table_permit() {
        let bindings = vec![
            binding(1, Scope::Table(200), Effect::Permit),
            binding(2, Scope::Namespace(1), Effect::Deny),
        ];
        let eff = resolve_effective_policy(&bindings, &target(1, 200, None));
        // Deny at the broader namespace scope wins over the table permit.
        assert_eq!(eff.decision, Effect::Deny);
        assert_eq!(eff.applicable, 2);
    }

    #[test]
    fn permits_and_their_row_predicates_together() {
        let mut b1 = binding(1, Scope::Namespace(1), Effect::Permit);
        b1.predicate_ref = Some(900);
        let mut b2 = binding(2, Scope::Table(200), Effect::Permit);
        b2.predicate_ref = Some(901);
        let eff = resolve_effective_policy(&[b1, b2], &target(1, 200, None));
        assert_eq!(eff.decision, Effect::Permit);
        // both row predicates are collected to be ANDed by FA
        assert_eq!(eff.predicate_refs, vec![900, 901]);
    }

    #[test]
    fn column_scope_only_applies_when_targeting_that_column() {
        let mut b = binding(
            1,
            Scope::Column {
                table: 200,
                column: 3,
            },
            Effect::Permit,
        );
        b.field_mask = Some(FieldMask::Redact);
        // targeting the whole table (column None): the column binding does not apply
        let table_eff = resolve_effective_policy(std::slice::from_ref(&b), &target(1, 200, None));
        assert_eq!(table_eff.applicable, 0);
        assert_eq!(table_eff.decision, Effect::Deny); // fail-closed
        // targeting column 3: it applies and contributes the mask
        let col_eff = resolve_effective_policy(&[b], &target(1, 200, Some(3)));
        assert_eq!(col_eff.applicable, 1);
        assert_eq!(col_eff.decision, Effect::Permit);
        assert_eq!(col_eff.field_masks, vec![(3, FieldMask::Redact)]);
    }

    #[test]
    fn unrelated_namespace_or_table_does_not_apply() {
        let bindings = vec![
            binding(1, Scope::Namespace(2), Effect::Permit), // different ns
            binding(2, Scope::Table(999), Effect::Permit),   // different table
        ];
        let eff = resolve_effective_policy(&bindings, &target(1, 200, None));
        assert_eq!(eff.applicable, 0);
        assert_eq!(eff.decision, Effect::Deny);
    }

    // --- subject attributes + the attribute-trust boundary ---

    #[test]
    fn claim_sourced_attribute_is_not_load_bearing() {
        // A tenant-controlled IdP forges clearance=TOP_SECRET as a CLAIM.
        let s = SubjectAttributes::new("alice", 7)
            .with_claim("clearance", AttrValue::Int(9))
            .with_claim("role", AttrValue::List(vec!["system_admin".into()]));
        // It is visible for display/audit...
        assert!(s.get("clearance").is_some());
        // ...but NOT load-bearing: a predicate reading it gets None, so the forged
        // clearance/role cannot satisfy any policy (closes admin-by-claims + the
        // Reference forged-attribute leak at the type level).
        assert_eq!(s.load_bearing("clearance"), None);
        assert_eq!(s.load_bearing("role"), None);
    }

    #[test]
    fn server_resolved_attribute_is_load_bearing() {
        let s = SubjectAttributes::new("bob", 7)
            .with_resolved("clearance", AttrValue::Int(3))
            .with_resolved("dept", AttrValue::Str("eng".into()));
        assert_eq!(s.load_bearing("clearance"), Some(&AttrValue::Int(3)));
        assert_eq!(s.load_bearing("dept"), Some(&AttrValue::Str("eng".into())));
    }

    #[test]
    fn a_claim_cannot_be_upgraded_by_coexisting_resolved_key() {
        // Same key: the last write wins per the bag; a claim overwriting a resolved
        // value (or vice-versa) carries its own source. Here the resolved value is
        // authoritative and load-bearing.
        let s = SubjectAttributes::new("carol", 7)
            .with_claim("region", AttrValue::Str("us".into()))
            .with_resolved("region", AttrValue::Str("eu".into()));
        assert_eq!(s.load_bearing("region"), Some(&AttrValue::Str("eu".into())));
        assert_eq!(s.get("region").unwrap().source, AttrSource::ServerResolved);
    }

    #[test]
    fn subject_is_tenant_scoped_and_bag_reports_size() {
        let s =
            SubjectAttributes::new("dan", 42).with_resolved("dept", AttrValue::Str("hr".into()));
        assert_eq!(s.subject_id, SubjectId("dan".into()));
        assert_eq!(s.tenant_stable_id, 42);
        assert_eq!(s.len(), 1);
        assert!(!s.is_empty());
        assert!(s.load_bearing("missing").is_none());
    }
}
