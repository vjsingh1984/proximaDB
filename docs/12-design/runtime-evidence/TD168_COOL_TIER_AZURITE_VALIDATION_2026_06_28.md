# TD-168 — Cool-tier validation against a real Azure backend (Azurite)

**Date:** 2026-06-28 · **Scope:** TD-168 Phase 2 (graph cold payloads → Cool tier)

## Why this runbook exists

The unit tests for `put_with_tier` / `ColdGraphRecordStore` run on `memory://`,
where the access tier is **meaningless and degrades to a plain put** — so they
cannot prove the `x-ms-access-tier: Cool` header is actually *accepted* by a real
Azure Blob endpoint. The failure mode they miss: a native-class mapping
regression (e.g. an invalid storage-class string) that a real Azure API rejects
with a 4xx. These integration tests close that gap against the **Azurite**
emulator. They are `#[cfg(feature = "azure")] #[ignore]` (no Azurite service runs
in CI), so they compile under `--features azure`/`cloud-full` but run only when
invoked manually with the env flag below.

## Tests

| Test | Crate | Proves |
|------|-------|--------|
| `put_with_tier_accepted_by_real_azure` | `proximadb-object-store` | the `x-ms-access-tier: Cool` PUT is accepted by a real Azure API + bytes round-trip; backend detected as `Azure` (tier path runs, not the Untiered degrade) |
| `cold_graph_record_store_round_trips_on_real_azure` | `proximadb` | the feature end-to-end: a canonical graph record is written Cool and round-trips by `oid` through the `ProximaRecordV2` wire on a real backend |

## Run it

```bash
# 1. Start Azurite (Blob service on :10000)
docker run --rm -p 10000:10000 mcr.microsoft.com/azure-storage/azurite \
  azurite-blob --blobHost 0.0.0.0

# 2. Create the container `proximadb-test` once (well-known emulator account).
#    e.g. with the Azure CLI pointed at the emulator:
az storage container create --name proximadb-test \
  --connection-string "DefaultEndpointsProtocol=http;AccountName=devstoreaccount1;AccountKey=Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==;BlobEndpoint=http://127.0.0.1:10000/devstoreaccount1;"

# 3. Run the ignored tests
PROXIMADB_AZURE_TEST=1 cargo test -p proximadb-object-store --features azure \
  -- --ignored put_with_tier_accepted_by_real_azure
PROXIMADB_AZURE_TEST=1 cargo test -p proximadb --features azure \
  -- --ignored cold_graph_record_store_round_trips_on_real_azure
```

## Reading the tier back (caveat)

`object_store` 0.13.2 sends `x-ms-access-tier` on the PUT
(`object_store::azure::client`, request path) but **does not surface it on read** —
`GetResult`/`ObjectMeta` attributes omit it (`client::get::get_attributes` parses
only CacheControl/Content-*/user-metadata). So these tests assert *acceptance* +
round-trip, not the resident tier value. To confirm the tier actually persisted,
query out-of-band:

```bash
az storage blob show --container-name proximadb-test --name cold/tier-probe.bin \
  --connection-string "...BlobEndpoint=http://127.0.0.1:10000/devstoreaccount1;" \
  --query properties.blobTier
# expected: "Cool"
```

A strong in-test tier read-back assertion would require the raw Azure SDK
(`azure_storage_blobs::BlobClient::get_properties().access_tier`) — deferred
(adds a dev-dep for one ignored test; tracked in TD-168 follow-ups).
