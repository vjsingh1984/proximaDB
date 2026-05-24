# Tier configuration contract

ProximaDB supports multi-tier tenant isolation via a JSON config file
loaded at startup. The file defines tier names, per-tier operational
caps (storage, ingest, scan, collection count), soft caps (search
quality knobs, SLA targets), and feature toggles (sync ingest, support
level, deployment class).

**This file is intentionally domain-neutral.** Operational concerns
only — no dollar amounts, no currency codes, no billing primitives, no
marketing copy. Commercial overlays (pricing, billing, customer-facing
tier display names) are the operator's responsibility and live in
operator-side artifacts.

## Where the config comes from

1. **Local file** (default): `/config/tier-config.json` (or the path
   `PROXIMADB_TIER_CONFIG_PATH` points at). If absent, ProximaDB starts
   in single-tier mode with sensible defaults.

2. **Remote fetch**: when `PROXIMADB_TIER_CONFIG_URL` is set, the
   `deploy/docker/entrypoint.sh` curl's that URL at container boot,
   atomic-replaces the local file, then falls back to the baked-in file
   if the fetch fails. Use cases:
   - Operator wants tier changes to propagate via deploy artifact
     rather than rebuilding the image.
   - Multi-cluster deployments share one canonical tier config from a
     CDN / object store / artifact registry.

3. **Single-tier mode** (no file, no URL): ProximaDB serves all
   tenants with default allowances. No quota enforcement on any tier
   dimension. Right for development, single-tenant deployments, and
   embedded use.

## Schema (v1)

```jsonc
{
  "schema_version": 1,            // currently 1; bumped on backward-incompatible changes
  "default_tier": "standard",     // tier_id assigned to new tenants without explicit override
  "tiers": [
    {
      "id": "standard",           // unique tier identifier; referenced from tenant metadata
      "description": "...",       // human-readable; optional
      "allowances": {             // hard caps; exceeded → rejected
        "storage_gb": 5,
        "ingest_gb_per_month": 2,
        "embedding_gb_per_month": 0.5,
        "scan_gb_per_month": 50000,
        "max_collections": 5
      },
      "soft_caps": {              // soft limits; exceeded → degraded service
        "scan_budget_gb": 5.0,
        "ef_search_cap": 128,
        "freshness_sla_seconds": 300
      },
      "features": {               // feature toggles per tier
        "sync_ingest": false,
        "support_level": "community",
        "deployment_class": "pooled"
      }
    }
  ]
}
```

### Field semantics

**`allowances`** — hard upper bounds. ProximaDB rejects writes / queries
that would exceed these. Tenant operators see HTTP 429 with a structured
error including the breached field and the cap value.

**`soft_caps`** — operational knobs that downgrade behavior rather than
reject:
- `scan_budget_gb` — query planner skips expensive scan paths above
  this per-query budget; results may be approximate.
- `ef_search_cap` — HNSW `ef` parameter ceiling; higher = better
  recall, lower = faster.
- `freshness_sla_seconds` — pacing target for async ingest visibility;
  tenants in this tier see records become searchable within this
  many seconds (best-effort).

**`features`** — boolean / enum toggles:
- `sync_ingest` (bool) — whether the tenant can use the synchronous
  ingest path (immediate consistency, higher per-byte cost).
- `support_level` (str) — operator-defined label; ProximaDB doesn't
  interpret beyond pass-through to observability.
- `deployment_class` (str) — same as above; pooled / dedicated /
  custom.

## Operator overlays (commercial concerns live OUTSIDE this contract)

ProximaDB intentionally doesn't model:
- Currency or money amounts
- Billing primitives (per-GB rates, per-call rates, monthly fees)
- Marketing copy (display names, tier descriptions intended for
  customer-facing UI, CTAs, feature comparison bullets)
- Trial periods, promotional credits, coupons

Operators carry those in their OWN artifacts. Example:

```text
operator-repo/
  pricing/
    tier-config.json        # mirrors this schema, fed to ProximaDB
    billing.json            # $ rates, currency, billing engine config (private)
    marketing-pricing.json  # display names, CTAs (drives marketing site)
```

The split keeps ProximaDB usable by any operator without inheriting
your specific commercial model. An evaluator running ProximaDB locally
sees only operational tiers; no dollars, no AnvaiOps branding, no
"Free Trial — $0 / 30 days" leaking into the engine codebase.

## Validation

A simple validation script ships in `scripts/validate_tier_config.py`
(when present). CI in operator repos should validate their
`tier-config.json` against this schema before publishing.

## Migration from the legacy `pricing.json` format

Older builds of ProximaDB read `/config/pricing.json` with a richer
schema (currency, dollar rates, marketing display fields). That file
remains supported for backward compatibility during the migration
window. New deployments should use `tier-config.json`; the next major
ProximaDB release will drop the `pricing.json` reader.

To migrate, strip your `pricing.json` to the fields documented above
and rename. The `scripts/strip_pricing_to_tier_config.py` helper
(when present) does this mechanically.
