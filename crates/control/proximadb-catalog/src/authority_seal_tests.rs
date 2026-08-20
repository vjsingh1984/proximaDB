// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! TD-CAT-7: the identity seal, proven.
//!
//! Three PRs fixed missing identity minting one backend at a time, and each fix
//! left the next instance alive. The structural claim of this change is that the
//! broken state is no longer *reachable by omission*:
//!
//! * a backend that claims identity authority cannot silently create a table
//!   without an `object_id` — [`Catalog::create_table`] rejects it;
//! * a backend that claims no authority is not asked to pretend;
//! * an authority's implementation is what answers, not a trait default.
//!
//! The tests below neuter exactly one mechanism each; if the post-condition or
//! the delegation is removed, they fail rather than passing vacuously.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::{
    Catalog, CatalogAuthority, CatalogIndex, CatalogNamespace, CatalogSchemaEvolution,
    CatalogTableSchema, CatalogTableStatistics, TableIdentifier,
};

/// The two axes that used to be indistinguishable: *does this backend claim to
/// mint identity*, and *did it actually mint one*.
struct StubCatalog {
    claims_authority: bool,
    mints_object_id: bool,
}

impl StubCatalog {
    fn new(claims_authority: bool, mints_object_id: bool) -> Self {
        Self {
            claims_authority,
            mints_object_id,
        }
    }
}

fn unused<T>(what: &str) -> Result<T> {
    Err(anyhow!(
        "StubCatalog::{what} is not exercised by these tests"
    ))
}

/// Deliberately inert — this models "an authority that forgot", which is the
/// state the seal has to catch. Only `max_object_id` answers, so the delegation
/// test has something to observe.
#[async_trait]
impl CatalogAuthority for StubCatalog {
    async fn max_object_id(&self) -> Result<Option<u64>> {
        Ok(Some(42))
    }
    async fn allocate_object_id(&self) -> Result<u64> {
        Ok(7)
    }
    async fn mint_collection_typed_identity(
        &self,
        _account: &str,
        _namespace_key: &str,
    ) -> Result<Option<(u32, u16, u32)>> {
        Ok(None)
    }
    async fn account_id_u32(&self, _account: &str) -> Result<Option<u32>> {
        Ok(None)
    }
    fn account_id_u32_lookup(&self, _account: &str) -> Option<u32> {
        None
    }
    async fn get_table_by_object_id(&self, _object_id: u64) -> Result<Option<TableIdentifier>> {
        Ok(None)
    }
    async fn get_namespace_by_object_id(&self, _object_id: u64) -> Result<Option<Vec<String>>> {
        Ok(None)
    }
}

#[async_trait]
impl Catalog for StubCatalog {
    fn identity_authority(&self) -> Option<&dyn CatalogAuthority> {
        if self.claims_authority {
            Some(self)
        } else {
            None
        }
    }

    fn name(&self) -> &str {
        "stub"
    }
    fn catalog_type(&self) -> &str {
        "stub"
    }

    async fn create_table_inner(
        &self,
        _identifier: &TableIdentifier,
        mut schema: CatalogTableSchema,
    ) -> Result<CatalogTableSchema> {
        if self.mints_object_id {
            schema.object_id = Some(7);
        }
        Ok(schema)
    }

    async fn create_namespace(
        &self,
        _namespace: &[String],
        _properties: HashMap<String, String>,
    ) -> Result<CatalogNamespace> {
        unused("create_namespace")
    }
    async fn drop_namespace(&self, _namespace: &[String], _cascade: bool) -> Result<bool> {
        unused("drop_namespace")
    }
    async fn list_namespaces(&self, _parent: Option<&[String]>) -> Result<Vec<CatalogNamespace>> {
        unused("list_namespaces")
    }
    async fn namespace_exists(&self, _namespace: &[String]) -> Result<bool> {
        unused("namespace_exists")
    }
    async fn get_namespace(&self, _namespace: &[String]) -> Result<CatalogNamespace> {
        unused("get_namespace")
    }
    async fn update_namespace_properties(
        &self,
        _namespace: &[String],
        _updates: HashMap<String, String>,
        _removals: Vec<String>,
    ) -> Result<()> {
        unused("update_namespace_properties")
    }
    async fn drop_table(&self, _identifier: &TableIdentifier, _purge: bool) -> Result<bool> {
        unused("drop_table")
    }
    async fn list_tables(&self, _namespace: &[String]) -> Result<Vec<TableIdentifier>> {
        unused("list_tables")
    }
    async fn table_exists(&self, _identifier: &TableIdentifier) -> Result<bool> {
        unused("table_exists")
    }
    async fn get_table(&self, _identifier: &TableIdentifier) -> Result<CatalogTableSchema> {
        unused("get_table")
    }
    async fn rename_table(&self, _from: &TableIdentifier, _to: &TableIdentifier) -> Result<()> {
        unused("rename_table")
    }
    async fn evolve_schema(
        &self,
        _identifier: &TableIdentifier,
        _evolution: CatalogSchemaEvolution,
    ) -> Result<CatalogTableSchema> {
        unused("evolve_schema")
    }
    async fn get_schema_version(&self, _identifier: &TableIdentifier) -> Result<i32> {
        unused("get_schema_version")
    }
    async fn get_schema_by_version(
        &self,
        _identifier: &TableIdentifier,
        _version: i32,
    ) -> Result<CatalogTableSchema> {
        unused("get_schema_by_version")
    }
    async fn create_index(
        &self,
        _identifier: &TableIdentifier,
        _index: CatalogIndex,
    ) -> Result<CatalogIndex> {
        unused("create_index")
    }
    async fn drop_index(&self, _identifier: &TableIdentifier, _index_name: &str) -> Result<bool> {
        unused("drop_index")
    }
    async fn list_indexes(&self, _identifier: &TableIdentifier) -> Result<Vec<CatalogIndex>> {
        unused("list_indexes")
    }
    async fn get_statistics(
        &self,
        _identifier: &TableIdentifier,
    ) -> Result<CatalogTableStatistics> {
        unused("get_statistics")
    }
    async fn update_statistics(
        &self,
        _identifier: &TableIdentifier,
        _stats: CatalogTableStatistics,
    ) -> Result<()> {
        unused("update_statistics")
    }
}

fn probe_table() -> (TableIdentifier, CatalogTableSchema) {
    (
        TableIdentifier::new(vec!["ns".to_string()], "t"),
        CatalogTableSchema::new("t"),
    )
}

/// The seal. An authority that returns a table with no `object_id` has produced
/// a row authorization cannot key on — that must fail at the write, not surface
/// later as an empty introspection column or a policy that matches nothing.
///
/// Teeth: delete the post-condition from `Catalog::create_table` and this test
/// goes green-by-omission, which is exactly the failure it exists to catch.
#[tokio::test]
async fn authority_that_does_not_mint_is_rejected() {
    let catalog = StubCatalog::new(true, false);
    let (identifier, schema) = probe_table();

    let err = catalog
        .create_table(&identifier, schema)
        .await
        .expect_err("an identity authority must not commit a table with no object_id");

    let message = err.to_string();
    assert!(
        message.contains("object_id"),
        "the rejection must name what is missing, got: {message}"
    );
}

/// The seal must not fire on the honest case, or backends would route around it.
#[tokio::test]
async fn authority_that_mints_is_accepted() {
    let catalog = StubCatalog::new(true, true);
    let (identifier, schema) = probe_table();

    let created = catalog
        .create_table(&identifier, schema)
        .await
        .expect("a minting authority must be accepted");

    assert_eq!(created.object_id, Some(7));
}

/// Federated/external metastores own no allocator. They never claimed to mint,
/// so the post-condition does not apply — that asymmetry is the whole reason
/// the trait was split rather than simply made stricter.
#[tokio::test]
async fn federated_catalog_without_object_id_is_accepted() {
    let catalog = StubCatalog::new(false, false);
    let (identifier, schema) = probe_table();

    let created = catalog
        .create_table(&identifier, schema)
        .await
        .expect("a catalog with no identity authority is exempt");

    assert_eq!(created.object_id, None);
}

/// `Catalog`'s identity methods are thin delegations, not answers of their own.
/// `StubCatalog` does not override `Catalog::max_object_id`, so a `42` reaching
/// a `Catalog`-typed call can only have come from its `CatalogAuthority` impl.
/// (Both traits are in scope here, so the call is spelled out — that is the
/// point of the assertion, not an accident.)
///
/// Teeth: restore the old `Ok(None)` default and this reads `None`.
#[tokio::test]
async fn catalog_identity_defaults_delegate_to_the_authority() {
    let authority = StubCatalog::new(true, true);
    assert_eq!(
        Catalog::max_object_id(&authority).await.expect("delegated"),
        Some(42),
        "the authority's answer must reach `Catalog` callers"
    );

    let federated = StubCatalog::new(false, true);
    assert_eq!(
        Catalog::max_object_id(&federated)
            .await
            .expect("no authority"),
        None,
        "a catalog with no authority still answers None — but now because it \
         has none, not because it forgot"
    );
}

/// The capability probe is the distinction that did not exist before: `None`
/// from a federated catalog and `None` from an authority that forgot used to be
/// the same value.
#[test]
fn identity_authority_probe_separates_cannot_from_did_not() {
    assert!(StubCatalog::new(true, false).identity_authority().is_some());
    assert!(
        StubCatalog::new(false, false)
            .identity_authority()
            .is_none()
    );
}
