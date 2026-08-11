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
    /// Many-to-one — the cardinality of a plain relational FK read in its
    /// natural direction (many referencing rows → one referenced row). Without
    /// it, FK derivation would have to invert `from`/`to` to reach `OneToMany`
    /// and so lose which endpoint declared the constraint.
    ManyToOne,
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
    /// How the relationship is materialized in the **relational** projection —
    /// the joining columns and their referential actions (D12-a: an FK and a
    /// graph edge are one object with different physical projections). `None`
    /// for a relationship with no relational projection (e.g. a native graph
    /// edge). Additive + `#[serde(default)]` so earlier persisted forms load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<RelationshipJoin>,
}

/// The relational projection of a [`RelationshipType`]: which columns join, and
/// what happens to the referencing rows when the referenced row changes.
///
/// Both column lists are [`ColumnRef`]s — resolved against their **own**
/// object's schema — so a relationship carries no name-keyed identity once it is
/// a catalog object. Deriving `to_columns` therefore requires resolving the
/// referenced table first (see [`DerivedRelationship::resolve`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationshipJoin {
    /// Referencing columns, in the `from` endpoint's object.
    pub from_columns: Vec<ColumnRef>,
    /// Referenced columns, in the `to` endpoint's object (same arity/order).
    pub to_columns: Vec<ColumnRef>,
    /// `ON DELETE` action, when the projection declares one.
    pub on_delete: Option<ReferentialAction>,
    /// `ON UPDATE` action, when the projection declares one.
    pub on_update: Option<ReferentialAction>,
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

use crate::{CatalogColumn, CatalogTableSchema, ColumnConstraint, ReferentialAction};

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
    // Discovery is delegated to the catalog's canonical accessor, which resolves
    // every location a key can be declared in (TD-CAT-6). This function keeps only
    // the *policy*: which declarations become which facet role.
    let identity = crate::schema::table_identity(schema);

    let primary_spec = if identity.primary_key.is_empty() {
        gaps.push("no natural primary key declared".to_string());
        None
    } else {
        let spec = CompositeKeySpec {
            keyspace: keyspace.clone(),
            columns: identity
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

    // 2. Every declared UNIQUE → UniquenessSecondary (only when a PK owns the row).
    //
    // A UNIQUE reaches the catalog by FOUR routes, and this step used to walk a
    // subset of them by hand — which is how it missed
    // `relational_capabilities.unique_indexes`, the route relational DDL actually
    // uses, until #1261. Discovery now comes from the canonical accessor, which
    // reads all of them; what stays here is the policy of turning declarations
    // into facets. The distinction matters: the identity slot is the seam by
    // which F2's ConditionalKeyStore learns which keys to fence (TF-2 §2 D12-b),
    // so an unseen UNIQUE is an unenforced one.
    for columns in identity.secondary_unique_sets() {
        let key = CompositeKeySpec {
            keyspace: keyspace.clone(),
            columns: columns.iter().map(|n| column_ref(schema, n)).collect(),
        };
        match &primary_spec {
            Some(pk) => facets.push(IdentityFacet {
                role: IdentityRole::UniquenessSecondary {
                    owning_primary: pk.clone(),
                },
                key,
            }),
            None => gaps.push(format!(
                "UNIQUE on ({}) has no primary key to own it",
                columns.join(", ")
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
// RelationshipType derivation from FK constraints (D12-a)
// ===========================================================================

/// One endpoint of a [`DerivedRelationship`], **before** it is a catalog object.
///
/// The whole point of the split: a foreign key names its target
/// (`references_table: String`), and a display name is *not* an identity
/// (ADR-075). Derivation therefore stops at [`Endpoint::Unresolved`] and hands
/// the name to a [`RelationshipEndpointResolver`] — it never mints or guesses an
/// `object_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Carries the authoritative stable id — ready to be a `RelationshipType`
    /// endpoint.
    Resolved(EntityRef),
    /// Name-only. Needs a catalog lookup, scoped to the owning tenant +
    /// namespace, to become an [`EntityRef`].
    Unresolved {
        /// The declared name (diagnostics + the resolver's lookup key; never an
        /// identity).
        name: String,
        /// The family the endpoint is expected to be.
        family: EntityFamily,
    },
}

impl Endpoint {
    /// The stable-id form, if this endpoint already has one.
    pub fn entity_ref(&self) -> Option<&EntityRef> {
        match self {
            Endpoint::Resolved(r) => Some(r),
            Endpoint::Unresolved { .. } => None,
        }
    }
}

/// Resolves a name-only [`Endpoint`] to its ADR-075 stable identity.
///
/// **Structurally tenant-scoped**: an implementation is built for exactly one
/// tenant and only sees that tenant's objects, so a cross-tenant FK cannot
/// resolve — it fails rather than producing a non-`Intra` relationship. Isolation
/// is structural, never a predicate the caller has to remember.
///
/// Every method is deny-biased: `None` means "not resolvable", and
/// [`DerivedRelationship::resolve`] turns that into an error — never a fabricated
/// or defaulted id.
pub trait RelationshipEndpointResolver {
    /// The tenant this resolver is bound to.
    fn tenant_stable_id(&self) -> TenantStableId;

    /// Look up a table by name within `namespace`. Returns `None` when the name
    /// is unknown, is in another namespace, or is in another tenant.
    fn resolve_table(&self, namespace: Option<NamespaceId>, table_name: &str) -> Option<EntityRef>;

    /// Look up a column's stable ordinal within an already-resolved table.
    fn resolve_column(&self, table: ObjectId, column_name: &str) -> Option<u32>;
}

/// Why a [`DerivedRelationship`] could not become a [`RelationshipType`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelationshipResolutionError {
    /// A name-only endpoint did not resolve in this tenant + namespace.
    UnresolvedEndpoint {
        /// The name that failed to resolve.
        name: String,
    },
    /// A joining column did not exist in the resolved table.
    UnresolvedColumn {
        /// The table the column was looked up in.
        table: ObjectId,
        /// The column name that failed to resolve.
        column: String,
    },
    /// The resolver is bound to a different tenant than the relationship.
    TenantMismatch {
        /// The relationship's tenant.
        expected: TenantStableId,
        /// The resolver's tenant.
        actual: TenantStableId,
    },
}

impl std::fmt::Display for RelationshipResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelationshipResolutionError::UnresolvedEndpoint { name } => write!(
                f,
                "relationship endpoint '{name}' does not resolve to a catalog object in this tenant/namespace"
            ),
            RelationshipResolutionError::UnresolvedColumn { table, column } => write!(
                f,
                "joining column '{column}' does not exist in catalog object {table}"
            ),
            RelationshipResolutionError::TenantMismatch { expected, actual } => write!(
                f,
                "resolver is bound to tenant {actual} but the relationship belongs to tenant {expected}"
            ),
        }
    }
}
impl std::error::Error for RelationshipResolutionError {}

/// A [`RelationshipType`] derived from a schema's FK constraints, **pending**
/// endpoint resolution and the minting of its own catalog id.
///
/// It holds everything the declaring schema knows and nothing it does not: the
/// `from` endpoint resolves immediately (we hold that schema), the `to` endpoint
/// and the referenced column ordinals do not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedRelationship {
    /// Owning tenant (stable id).
    pub tenant_stable_id: TenantStableId,
    /// The namespace both endpoints must live in (ADR-074). `None` for a
    /// pre-namespace legacy schema.
    pub namespace: Option<NamespaceId>,
    /// The referencing side — the table that declared the FK.
    pub from: Endpoint,
    /// The referenced side — named by the FK, usually unresolved.
    pub to: Endpoint,
    /// Always [`Direction::Directed`] for an FK (referencing → referenced).
    pub direction: Direction,
    /// [`Multiplicity::OneToOne`] when the FK columns are themselves unique in
    /// the referencing table, else [`Multiplicity::ManyToOne`].
    pub multiplicity: Multiplicity,
    /// Referencing columns, resolved against the declaring schema.
    pub from_columns: Vec<ColumnRef>,
    /// Referenced column **names** — their ordinals live in the target's schema,
    /// so they stay names until [`DerivedRelationship::resolve`] runs.
    pub to_column_names: Vec<String>,
    /// `ON DELETE` action declared by the FK.
    pub on_delete: Option<ReferentialAction>,
    /// `ON UPDATE` action declared by the FK.
    pub on_update: Option<ReferentialAction>,
    /// Cross-tenant policy — always [`RelationshipTenancy::Intra`]: an FK names
    /// a table in its own catalog scope, and the resolver cannot cross tenants.
    pub tenancy: RelationshipTenancy,
}

impl DerivedRelationship {
    /// Complete this into a first-class [`RelationshipType`], given the
    /// `object_id` the catalog minted for it and a tenant-scoped resolver.
    ///
    /// Fail-closed: any endpoint or column that does not resolve is an `Err` —
    /// a half-resolved or id-guessing relationship is never produced.
    pub fn resolve(
        &self,
        object_id: ObjectId,
        resolver: &dyn RelationshipEndpointResolver,
    ) -> Result<RelationshipType, RelationshipResolutionError> {
        if resolver.tenant_stable_id() != self.tenant_stable_id {
            return Err(RelationshipResolutionError::TenantMismatch {
                expected: self.tenant_stable_id,
                actual: resolver.tenant_stable_id(),
            });
        }

        let resolve_endpoint = |e: &Endpoint| -> Result<EntityRef, RelationshipResolutionError> {
            match e {
                Endpoint::Resolved(r) => Ok(r.clone()),
                // The resolver, not the declaring DDL, is authoritative for the
                // resolved object's family — an FK's target may be projected
                // from a non-relational family (D12-a).
                Endpoint::Unresolved { name, .. } => {
                    resolver.resolve_table(self.namespace, name).ok_or_else(|| {
                        RelationshipResolutionError::UnresolvedEndpoint { name: name.clone() }
                    })
                }
            }
        };

        let from = resolve_endpoint(&self.from)?;
        let to = resolve_endpoint(&self.to)?;

        let to_columns = self
            .to_column_names
            .iter()
            .map(|name| {
                resolver
                    .resolve_column(to.object_id, name)
                    .map(|ordinal| ColumnRef {
                        ordinal,
                        name: name.clone(),
                    })
                    .ok_or_else(|| RelationshipResolutionError::UnresolvedColumn {
                        table: to.object_id,
                        column: name.clone(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(RelationshipType {
            object_id,
            tenant_stable_id: self.tenant_stable_id,
            from,
            to,
            direction: self.direction,
            multiplicity: self.multiplicity,
            // An FK edge carries no properties of its own; its columns are the
            // join, recorded below rather than smuggled in as attributes.
            attributes: Vec::new(),
            tenancy: self.tenancy,
            join: Some(RelationshipJoin {
                from_columns: self.from_columns.clone(),
                to_columns,
                on_delete: self.on_delete,
                on_update: self.on_update,
            }),
        })
    }
}

/// The output of [`relationships_from_table_schema`] — derived relationships plus
/// an explicit account of what could **not** be modeled, so a malformed or
/// legacy constraint is never silently dropped (same discipline as
/// [`IdentitySlot::completeness_gaps`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DerivedRelationships {
    /// One per modelable FK constraint.
    pub relationships: Vec<DerivedRelationship>,
    /// Human-readable reasons for each constraint that was skipped.
    pub gaps: Vec<String>,
}

use crate::schema::same_column_set;

/// Is this column set unique **in the declaring table** (its PK, a unique index,
/// or a `UNIQUE` constraint)? Order-insensitive: `UNIQUE(a,b)` fences `(b,a)` too.
fn columns_are_unique_in(schema: &CatalogTableSchema, columns: &[String]) -> bool {
    let identity = crate::schema::table_identity(schema);
    same_column_set(columns, &identity.primary_key)
        || identity
            .unique_sets
            .iter()
            .any(|set| same_column_set(set, columns))
}

/// Derive a [`DerivedRelationship`] per foreign key on `schema` (D12-a).
///
/// A relational FK and a graph edge are the *same* metamodel object; this is the
/// seam that proves it, by lifting `relational_capabilities.constraints` into the
/// first-class relationship type instead of leaving edges implicit in DDL.
///
/// What derivation deliberately does **not** do:
///
/// - **Mint or guess ids.** The `to` endpoint stays [`Endpoint::Unresolved`]
///   because an FK names its target; `resolve` turns the name into a stable id
///   through a tenant-scoped catalog lookup, or fails.
/// - **Assume the declaring table has an identity.** A legacy schema with no
///   `object_id` yields an unresolved `from` endpoint too, recorded as such.
/// - **Model `CHECK`/`UNIQUE`.** Those are not relationships; `UNIQUE` feeds
///   [`identity_slot_from_table_schema`] instead.
///
/// Everything unmodelable lands in [`DerivedRelationships::gaps`].
pub fn relationships_from_table_schema(
    schema: &CatalogTableSchema,
    tenant_stable_id: TenantStableId,
) -> DerivedRelationships {
    let mut out = DerivedRelationships::default();

    let from = match schema.object_id {
        Some(object_id) => Endpoint::Resolved(EntityRef {
            object_id,
            family: EntityFamily::Relational,
        }),
        None => Endpoint::Unresolved {
            name: schema.name.clone(),
            family: EntityFamily::Relational,
        },
    };

    for constraint in &schema.relational_capabilities.constraints {
        let ColumnConstraint::ForeignKey {
            columns,
            references_table,
            references_columns,
            on_delete,
            on_update,
        } = constraint
        else {
            continue; // Unique/Check are not relationships.
        };

        if columns.is_empty() || references_columns.is_empty() {
            out.gaps.push(format!(
                "foreign key to '{references_table}' declares no columns on one side; not modelable as a relationship"
            ));
            continue;
        }
        if columns.len() != references_columns.len() {
            out.gaps.push(format!(
                "foreign key to '{}' has mismatched arity ({} referencing vs {} referenced columns)",
                references_table,
                columns.len(),
                references_columns.len()
            ));
            continue;
        }
        if references_table.is_empty() {
            out.gaps
                .push("foreign key names no referenced table".to_string());
            continue;
        }

        out.relationships.push(DerivedRelationship {
            tenant_stable_id,
            namespace: schema.stable_namespace_id,
            from: from.clone(),
            to: Endpoint::Unresolved {
                name: references_table.clone(),
                family: EntityFamily::Relational,
            },
            direction: Direction::Directed,
            multiplicity: if columns_are_unique_in(schema, columns) {
                Multiplicity::OneToOne
            } else {
                Multiplicity::ManyToOne
            },
            from_columns: columns.iter().map(|n| column_ref(schema, n)).collect(),
            to_column_names: references_columns.clone(),
            on_delete: *on_delete,
            on_update: *on_update,
            tenancy: RelationshipTenancy::Intra,
        });
    }

    out
}

/// A [`RelationshipEndpointResolver`] over a set of already-loaded schemas for
/// **one** tenant — the resolution hook the catalog supplies when it has the
/// namespace's schemas in hand.
///
/// Namespace matching is exact `Option<NamespaceId>` equality, with no widening
/// or fallback: `Some(3)` never matches `Some(4)` or `None`, so a relationship
/// cannot silently reach across the ADR-074 isolation boundary. (`None` matching
/// `None` is the pre-namespace legacy scope, not a wildcard.)
pub struct CatalogSchemaResolver {
    tenant_stable_id: TenantStableId,
    tables: BTreeMap<(Option<NamespaceId>, String), EntityRef>,
    columns: BTreeMap<(ObjectId, String), u32>,
}

impl CatalogSchemaResolver {
    /// Build a resolver over `schemas`, all belonging to `tenant_stable_id`.
    /// Schemas without an `object_id` are skipped — they have no identity to
    /// resolve *to*.
    pub fn new<'a>(
        tenant_stable_id: TenantStableId,
        schemas: impl IntoIterator<Item = &'a CatalogTableSchema>,
    ) -> Self {
        let mut tables = BTreeMap::new();
        let mut columns = BTreeMap::new();
        for schema in schemas {
            let Some(object_id) = schema.object_id else {
                continue;
            };
            tables.insert(
                (schema.stable_namespace_id, schema.name.clone()),
                EntityRef {
                    object_id,
                    family: EntityFamily::Relational,
                },
            );
            for col in &schema.columns {
                if !col.is_deleted {
                    columns.insert((object_id, col.name.clone()), col.id.max(0) as u32);
                }
            }
        }
        Self {
            tenant_stable_id,
            tables,
            columns,
        }
    }
}

impl RelationshipEndpointResolver for CatalogSchemaResolver {
    fn tenant_stable_id(&self) -> TenantStableId {
        self.tenant_stable_id
    }

    fn resolve_table(&self, namespace: Option<NamespaceId>, table_name: &str) -> Option<EntityRef> {
        self.tables
            .get(&(namespace, table_name.to_string()))
            .cloned()
    }

    fn resolve_column(&self, table: ObjectId, column_name: &str) -> Option<u32> {
        self.columns.get(&(table, column_name.to_string())).copied()
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

impl Target {
    /// Build a `Target` from a catalog object id, refusing rather than truncating.
    ///
    /// `CollectionId` is `u32` while catalog object ids come from a global
    /// monotonic **u64** allocator. Casting `as u32` silently wraps, and
    /// [`scope_covers`] compares with `==` — so a wrapped id can COLLIDE with a
    /// legitimately-bound table and be authorized by someone else's policy or
    /// grant. That is an authorization-correctness bug, not a lossy cast.
    ///
    /// Widening `CollectionId` to u64 is the eventual fix but is a persisted
    /// format migration: `Scope` is serde-persisted in policy bindings and
    /// `grants.json`, so a widened writer would emit values an older reader
    /// cannot parse. Until then this constructor makes the unrepresentable case
    /// **fail closed** — callers map `None` onto their existing deny path, so an
    /// id that cannot be represented can never be authorized.
    pub fn try_from_object_ids(namespace: NamespaceId, object_id: u64) -> Option<Self> {
        let table = CollectionId::try_from(object_id).ok()?;
        Some(Self {
            namespace,
            table,
            column: None,
        })
    }
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
pub(crate) fn scope_covers(scope: &Scope, target: &Target) -> bool {
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
    /// B3: a `Target` must never be built by truncation. `scope_covers` matches
    /// with `==`, so a wrapped object id could collide with a legitimately-bound
    /// table and be authorized by someone else's binding.
    #[test]
    fn target_refuses_object_ids_that_do_not_fit() {
        // Representable ids build normally.
        let ok = Target::try_from_object_ids(0, 42).expect("small id fits");
        assert_eq!(ok.table, 42);
        let max = Target::try_from_object_ids(0, u32::MAX as u64).expect("u32::MAX fits");
        assert_eq!(max.table, u32::MAX);

        // One past the boundary must REFUSE, not wrap to 0.
        assert_eq!(
            Target::try_from_object_ids(0, u32::MAX as u64 + 1),
            None,
            "an id one past u32::MAX must fail closed, not alias to 0"
        );

        // The aliasing pair the old `as u32` produced: N and N + 2^32 both
        // truncated to N. They must not both resolve to the same target.
        let a = Target::try_from_object_ids(0, 7);
        let aliased = Target::try_from_object_ids(0, 7u64 + (1u64 << 32));
        assert!(a.is_some());
        assert_eq!(aliased, None, "the 2^32 alias must be refused outright");
    }

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
            join: None,
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

    fn secondary_key_names(slot: &IdentitySlot) -> Vec<Vec<String>> {
        slot.facets
            .iter()
            .filter(|f| matches!(f.role, IdentityRole::UniquenessSecondary { .. }))
            .map(|f| f.key.columns.iter().map(|c| c.name.clone()).collect())
            .collect()
    }

    /// `t(id PK, email, tenant, region)` with UNIQUE declared as a *constraint*
    /// rather than an index.
    fn schema_with_unique_constraints(constraints: Vec<ColumnConstraint>) -> CatalogTableSchema {
        CatalogTableSchema::new("t")
            .with_column(CatalogColumn::new(
                0,
                "id",
                proximadb_data_model::ProximaType::Int64,
            ))
            .with_column(str_col(1, "email"))
            .with_column(str_col(2, "tenant"))
            .with_column(str_col(3, "region"))
            .with_primary_key(vec!["id".to_string()])
            .with_relational_capabilities(crate::RelationalCapabilities {
                constraints,
                ..Default::default()
            })
    }

    #[test]
    fn a_unique_declared_as_a_constraint_is_fenced_like_one_declared_as_an_index() {
        // The gap: a UNIQUE reaches the catalog by two routes, and derivation
        // read only the index route. The identity slot is how F2's CKS learns
        // which keys to fence, so a UNIQUE it cannot see is a UNIQUE that is
        // never enforced.
        let slot = identity_slot_from_table_schema(&schema_with_unique_constraints(vec![
            ColumnConstraint::Unique {
                columns: vec!["email".to_string()],
            },
        ]));

        assert_eq!(secondary_key_names(&slot), vec![vec!["email".to_string()]]);
        let secondary = slot
            .facets
            .iter()
            .find(|f| matches!(f.role, IdentityRole::UniquenessSecondary { .. }))
            .expect("the constraint-declared UNIQUE produces a facet");
        assert!(secondary.is_cks_fenced());
        match &secondary.role {
            IdentityRole::UniquenessSecondary { owning_primary } => {
                assert_eq!(owning_primary.columns[0].name, "id");
            }
            _ => unreachable!(),
        }
        assert!(slot.validate().is_ok());
    }

    #[test]
    fn a_composite_unique_constraint_keeps_all_its_columns() {
        let slot = identity_slot_from_table_schema(&schema_with_unique_constraints(vec![
            ColumnConstraint::Unique {
                columns: vec!["tenant".to_string(), "email".to_string()],
            },
        ]));
        assert_eq!(
            secondary_key_names(&slot),
            vec![vec!["tenant".to_string(), "email".to_string()]]
        );
    }

    #[test]
    fn the_same_key_declared_as_both_an_index_and_a_constraint_yields_one_facet() {
        // Catalogs routinely carry both forms for one DDL `UNIQUE`. Two facets
        // for one key would mean the CKS fences it twice.
        let schema = schema_with_unique_constraints(vec![ColumnConstraint::Unique {
            columns: vec!["email".to_string()],
        }])
        .with_index(
            crate::CatalogIndex::new(
                "uq_email",
                vec!["email".to_string()],
                crate::CatalogIndexType::BTree,
            )
            .unique(),
        );
        assert_eq!(
            secondary_key_names(&identity_slot_from_table_schema(&schema)),
            vec![vec!["email".to_string()]]
        );
    }

    #[test]
    fn column_order_does_not_make_a_second_key() {
        // UNIQUE(a,b) and UNIQUE(b,a) fence the same tuple.
        let schema = schema_with_unique_constraints(vec![ColumnConstraint::Unique {
            columns: vec!["tenant".to_string(), "email".to_string()],
        }])
        .with_index(
            crate::CatalogIndex::new(
                "uq_email_tenant",
                vec!["email".to_string(), "tenant".to_string()],
                crate::CatalogIndexType::BTree,
            )
            .unique(),
        );
        assert_eq!(
            secondary_key_names(&identity_slot_from_table_schema(&schema)).len(),
            1
        );
    }

    #[test]
    fn a_unique_declared_inline_in_create_table_is_fenced() {
        // The dominant route: `CREATE TABLE t (..., UNIQUE (email))` is lowered
        // into relational_capabilities.unique_indexes, not schema.indexes.
        let schema = CatalogTableSchema::new("t")
            .with_column(CatalogColumn::new(
                0,
                "id",
                proximadb_data_model::ProximaType::Int64,
            ))
            .with_column(str_col(1, "email"))
            .with_primary_key(vec!["id".to_string()])
            .with_relational_capabilities(crate::RelationalCapabilities {
                unique_indexes: vec![crate::CatalogIndex::new(
                    "unique_email",
                    vec!["email".to_string()],
                    crate::CatalogIndexType::BTree,
                )],
                ..Default::default()
            });

        assert_eq!(
            secondary_key_names(&identity_slot_from_table_schema(&schema)),
            vec![vec!["email".to_string()]],
            "an inline UNIQUE must produce a CKS-fenced facet"
        );
    }

    #[test]
    fn a_unique_index_entry_is_not_filtered_on_is_unique() {
        // Entries in `unique_indexes` are unique by virtue of the field they sit
        // in; `CatalogIndex::new` leaves `is_unique` false, and DDL does not set
        // it. Filtering on the flag here would silently drop every DDL UNIQUE.
        let idx = crate::CatalogIndex::new(
            "unique_email",
            vec!["email".to_string()],
            crate::CatalogIndexType::BTree,
        );
        assert!(!idx.is_unique, "precondition: DDL leaves the flag unset");

        let schema = CatalogTableSchema::new("t")
            .with_column(CatalogColumn::new(
                0,
                "id",
                proximadb_data_model::ProximaType::Int64,
            ))
            .with_column(str_col(1, "email"))
            .with_primary_key(vec!["id".to_string()])
            .with_relational_capabilities(crate::RelationalCapabilities {
                unique_indexes: vec![idx],
                ..Default::default()
            });
        assert_eq!(
            secondary_key_names(&identity_slot_from_table_schema(&schema)).len(),
            1
        );
    }

    #[test]
    fn the_metamodel_agrees_with_the_live_unique_key_discovery() {
        // The live enforcement path (services/record_store.rs::schema_unique_column_sets)
        // reads unique_indexes + constraints. The metamodel must not disagree,
        // or F2's CKS and the identity slot would fence different key sets once
        // D12-b wires the slot as the seam.
        let schema = CatalogTableSchema::new("t")
            .with_column(CatalogColumn::new(
                0,
                "id",
                proximadb_data_model::ProximaType::Int64,
            ))
            .with_column(str_col(1, "email"))
            .with_column(str_col(2, "tenant"))
            .with_primary_key(vec!["id".to_string()])
            .with_relational_capabilities(crate::RelationalCapabilities {
                unique_indexes: vec![crate::CatalogIndex::new(
                    "unique_email",
                    vec!["email".to_string()],
                    crate::CatalogIndexType::BTree,
                )],
                constraints: vec![ColumnConstraint::Unique {
                    columns: vec!["tenant".to_string()],
                }],
                ..Default::default()
            });

        // Mirrors schema_unique_column_sets: unique_indexes ++ Unique constraints.
        let mut live: Vec<Vec<String>> = schema
            .relational_capabilities
            .unique_indexes
            .iter()
            .map(|i| i.columns.clone())
            .chain(
                schema
                    .relational_capabilities
                    .constraints
                    .iter()
                    .filter_map(|c| match c {
                        ColumnConstraint::Unique { columns } => Some(columns.clone()),
                        _ => None,
                    }),
            )
            .collect();
        let mut derived = secondary_key_names(&identity_slot_from_table_schema(&schema));
        live.sort();
        derived.sort();
        assert_eq!(derived, live, "metamodel and live enforcement disagree");
    }

    #[test]
    fn a_unique_constraint_restating_the_primary_key_is_not_a_secondary() {
        let slot = identity_slot_from_table_schema(&schema_with_unique_constraints(vec![
            ColumnConstraint::Unique {
                columns: vec!["id".to_string()],
            },
        ]));
        assert!(secondary_key_names(&slot).is_empty());
        assert_eq!(slot.primary_count(), 1);
    }

    #[test]
    fn non_unique_constraints_produce_no_facet() {
        let slot = identity_slot_from_table_schema(&schema_with_unique_constraints(vec![
            ColumnConstraint::Check {
                expression: "email <> ''".to_string(),
            },
            ColumnConstraint::ForeignKey {
                columns: vec!["tenant".to_string()],
                references_table: "tenants".to_string(),
                references_columns: vec!["id".to_string()],
                on_delete: None,
                on_update: None,
            },
        ]));
        assert!(secondary_key_names(&slot).is_empty());
    }

    #[test]
    fn a_constraint_unique_without_a_primary_key_is_recorded_as_a_gap() {
        let schema = CatalogTableSchema::new("t")
            .with_column(str_col(0, "email"))
            .with_relational_capabilities(crate::RelationalCapabilities {
                constraints: vec![ColumnConstraint::Unique {
                    columns: vec!["email".to_string()],
                }],
                ..Default::default()
            });
        let slot = identity_slot_from_table_schema(&schema);
        // No PK to own it — say so rather than emitting an unowned facet.
        assert!(secondary_key_names(&slot).is_empty());
        // Unified discovery deliberately drops route attribution ("index" vs
        // "constraint") — the columns identify the declaration, and which of the
        // four fields it happened to be written to is not actionable.
        assert!(
            slot.completeness_gaps
                .iter()
                .any(|g| g.contains("UNIQUE on (email)") && g.contains("no primary key"))
        );
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

    // --- RelationshipType derivation from FK constraints (D12-a) ---

    const NS: NamespaceId = 3;
    const TENANT: TenantStableId = 7;

    fn str_col(id: i32, name: &str) -> CatalogColumn {
        CatalogColumn::new(id, name, proximadb_data_model::ProximaType::String)
    }

    /// `customer(c_custkey PK, c_name)` — the FK target.
    fn customer(object_id: u64) -> CatalogTableSchema {
        let mut s = CatalogTableSchema::new("customer")
            .with_column(CatalogColumn::new(
                0,
                "c_custkey",
                proximadb_data_model::ProximaType::Int64,
            ))
            .with_column(str_col(1, "c_name"))
            .with_primary_key(vec!["c_custkey".to_string()]);
        s.object_id = Some(object_id);
        s.stable_namespace_id = Some(NS);
        s
    }

    /// `orders(o_orderkey PK, o_custkey → customer.c_custkey)`.
    fn orders(object_id: u64, fk_on_delete: Option<ReferentialAction>) -> CatalogTableSchema {
        let mut s = CatalogTableSchema::new("orders")
            .with_column(CatalogColumn::new(
                0,
                "o_orderkey",
                proximadb_data_model::ProximaType::Int64,
            ))
            .with_column(CatalogColumn::new(
                1,
                "o_custkey",
                proximadb_data_model::ProximaType::Int64,
            ))
            .with_primary_key(vec!["o_orderkey".to_string()])
            .with_relational_capabilities(crate::RelationalCapabilities {
                constraints: vec![ColumnConstraint::ForeignKey {
                    columns: vec!["o_custkey".to_string()],
                    references_table: "customer".to_string(),
                    references_columns: vec!["c_custkey".to_string()],
                    on_delete: fk_on_delete,
                    on_update: None,
                }],
                ..Default::default()
            });
        s.object_id = Some(object_id);
        s.stable_namespace_id = Some(NS);
        s
    }

    #[test]
    fn foreign_key_derives_a_many_to_one_relationship_with_an_unresolved_target() {
        let schema = orders(200, Some(ReferentialAction::Cascade));
        let derived = relationships_from_table_schema(&schema, TENANT);

        assert_eq!(derived.relationships.len(), 1);
        assert!(derived.gaps.is_empty());
        let r = &derived.relationships[0];

        // The declaring side has an identity; the target is NAME-only — the FK
        // gives us a display name, and a name is never an identity (ADR-075).
        assert_eq!(r.from.entity_ref().map(|e| e.object_id), Some(200));
        assert!(matches!(
            &r.to,
            Endpoint::Unresolved { name, .. } if name == "customer"
        ));

        assert_eq!(r.direction, Direction::Directed);
        assert_eq!(r.multiplicity, Multiplicity::ManyToOne);
        assert_eq!(r.tenancy, RelationshipTenancy::Intra);
        assert_eq!(r.namespace, Some(NS));
        assert_eq!(r.from_columns[0].name, "o_custkey");
        assert_eq!(r.from_columns[0].ordinal, 1); // the schema's stable field id
        assert_eq!(r.to_column_names, vec!["c_custkey".to_string()]);
        assert_eq!(r.on_delete, Some(ReferentialAction::Cascade));
    }

    #[test]
    fn a_unique_foreign_key_column_derives_one_to_one() {
        // Same FK, but o_custkey is itself UNIQUE in orders → at most one order
        // per customer.
        let mut schema = orders(200, None);
        schema = schema.with_index(
            crate::CatalogIndex::new(
                "uq_o_custkey",
                vec!["o_custkey".to_string()],
                crate::CatalogIndexType::BTree,
            )
            .unique(),
        );
        let derived = relationships_from_table_schema(&schema, TENANT);
        assert_eq!(
            derived.relationships[0].multiplicity,
            Multiplicity::OneToOne
        );
    }

    #[test]
    fn resolve_completes_the_endpoint_and_the_join_columns() {
        let derived = relationships_from_table_schema(&orders(200, None), TENANT);
        let resolver = CatalogSchemaResolver::new(TENANT, [&customer(100), &orders(200, None)]);

        // 900 is the id the *catalog* mints for the relationship object; derivation
        // never invents one.
        let r = derived.relationships[0]
            .resolve(900, &resolver)
            .expect("resolves within the tenant + namespace");

        assert_eq!(r.object_id, 900);
        assert_eq!(r.tenant_stable_id, TENANT);
        assert_eq!(r.from.object_id, 200);
        assert_eq!(r.to.object_id, 100);
        assert!(r.tenancy_is_admissible());
        // An FK edge carries no properties of its own — the columns are the join.
        assert!(r.attributes.is_empty());

        let join = r.join.expect("FK relationship has a relational projection");
        assert_eq!(join.from_columns[0].name, "o_custkey");
        assert_eq!(join.to_columns[0].name, "c_custkey");
        // The referenced ordinal came from customer's schema, not orders'.
        assert_eq!(join.to_columns[0].ordinal, 0);
    }

    #[test]
    fn resolve_denies_an_unknown_target_instead_of_fabricating_an_id() {
        let derived = relationships_from_table_schema(&orders(200, None), TENANT);
        // customer is absent from the resolver's world.
        let resolver = CatalogSchemaResolver::new(TENANT, [&orders(200, None)]);

        assert_eq!(
            derived.relationships[0].resolve(900, &resolver),
            Err(RelationshipResolutionError::UnresolvedEndpoint {
                name: "customer".to_string()
            })
        );
    }

    #[test]
    fn resolve_denies_across_the_namespace_boundary() {
        let derived = relationships_from_table_schema(&orders(200, None), TENANT);
        // A `customer` table exists for this tenant, but in another namespace.
        let mut other_ns_customer = customer(100);
        other_ns_customer.stable_namespace_id = Some(NS + 1);
        let resolver = CatalogSchemaResolver::new(TENANT, [&other_ns_customer]);

        // ADR-074 isolation is structural: the name matches, the namespace does
        // not, so the relationship does not form.
        assert!(matches!(
            derived.relationships[0].resolve(900, &resolver),
            Err(RelationshipResolutionError::UnresolvedEndpoint { .. })
        ));
    }

    #[test]
    fn resolve_denies_a_resolver_bound_to_another_tenant() {
        let derived = relationships_from_table_schema(&orders(200, None), TENANT);
        let foreign = CatalogSchemaResolver::new(TENANT + 1, [&customer(100), &orders(200, None)]);

        assert_eq!(
            derived.relationships[0].resolve(900, &foreign),
            Err(RelationshipResolutionError::TenantMismatch {
                expected: TENANT,
                actual: TENANT + 1,
            })
        );
    }

    #[test]
    fn resolve_denies_when_a_referenced_column_is_missing() {
        let derived = relationships_from_table_schema(&orders(200, None), TENANT);
        // A `customer` whose PK column was renamed away under the FK.
        let mut stale = CatalogTableSchema::new("customer").with_column(str_col(0, "other"));
        stale.object_id = Some(100);
        stale.stable_namespace_id = Some(NS);
        let resolver = CatalogSchemaResolver::new(TENANT, [&stale]);

        assert_eq!(
            derived.relationships[0].resolve(900, &resolver),
            Err(RelationshipResolutionError::UnresolvedColumn {
                table: 100,
                column: "c_custkey".to_string(),
            })
        );
    }

    #[test]
    fn unique_and_check_constraints_are_not_relationships() {
        let schema = CatalogTableSchema::new("t")
            .with_column(str_col(0, "a"))
            .with_relational_capabilities(crate::RelationalCapabilities {
                constraints: vec![
                    ColumnConstraint::Unique {
                        columns: vec!["a".to_string()],
                    },
                    ColumnConstraint::Check {
                        expression: "a <> ''".to_string(),
                    },
                ],
                ..Default::default()
            });
        let derived = relationships_from_table_schema(&schema, TENANT);
        assert!(derived.relationships.is_empty());
        assert!(derived.gaps.is_empty()); // not a gap — they are simply not edges
    }

    #[test]
    fn a_malformed_foreign_key_is_recorded_as_a_gap_not_silently_dropped() {
        let schema = CatalogTableSchema::new("t")
            .with_column(str_col(0, "a"))
            .with_relational_capabilities(crate::RelationalCapabilities {
                constraints: vec![
                    ColumnConstraint::ForeignKey {
                        columns: vec!["a".to_string(), "b".to_string()],
                        references_table: "u".to_string(),
                        references_columns: vec!["x".to_string()], // arity mismatch
                        on_delete: None,
                        on_update: None,
                    },
                    ColumnConstraint::ForeignKey {
                        columns: vec![], // no columns on the referencing side
                        references_table: "u".to_string(),
                        references_columns: vec!["x".to_string()],
                        on_delete: None,
                        on_update: None,
                    },
                ],
                ..Default::default()
            });
        let derived = relationships_from_table_schema(&schema, TENANT);
        assert!(derived.relationships.is_empty());
        assert_eq!(derived.gaps.len(), 2);
        assert!(derived.gaps.iter().any(|g| g.contains("mismatched arity")));
        assert!(derived.gaps.iter().any(|g| g.contains("no columns")));
    }

    #[test]
    fn a_legacy_schema_without_an_object_id_leaves_the_from_endpoint_unresolved() {
        let mut schema = orders(200, None);
        schema.object_id = None; // pre-ADR-031 persisted schema
        let derived = relationships_from_table_schema(&schema, TENANT);

        // No identity to stand on, so we say so rather than inventing one.
        assert!(matches!(
            &derived.relationships[0].from,
            Endpoint::Unresolved { name, .. } if name == "orders"
        ));
        // And it stays unresolvable: a schema with no object_id is not in the
        // resolver's table either.
        let resolver = CatalogSchemaResolver::new(TENANT, [&customer(100), &schema]);
        assert!(matches!(
            derived.relationships[0].resolve(900, &resolver),
            Err(RelationshipResolutionError::UnresolvedEndpoint { .. })
        ));
    }

    #[test]
    fn a_resolved_relationship_round_trips_through_serde() {
        let derived = relationships_from_table_schema(&orders(200, None), TENANT);
        let resolver = CatalogSchemaResolver::new(TENANT, [&customer(100), &orders(200, None)]);
        let r = derived.relationships[0]
            .resolve(900, &resolver)
            .expect("resolves");

        let json = serde_json::to_string(&r).expect("serializes");
        let back: RelationshipType = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, r);

        // `join` is additive: a form persisted before it existed still loads.
        let older = json.replace(",\"join\":", ",\"unused\":");
        let legacy: RelationshipType =
            serde_json::from_str(&older).expect("legacy form without `join` deserializes");
        assert!(legacy.join.is_none());
    }
}
