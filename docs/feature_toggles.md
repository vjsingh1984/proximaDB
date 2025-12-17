Feature toggles for optional surfaces
=====================================

- `ai_endpoints`: enables `/ai/*` REST routes; initialization runs on startup. Turn on via `cargo run --features ai_endpoints`.
- `sales_endpoints`: enables `/sales/*` REST routes; initialization runs on startup. Turn on via `cargo run --features sales_endpoints`.
- `tenant_access`: compiles the tenant access service; export from `services::tenant_access`.
- `executive_intel`: compiles the executive intelligence module; opt-in to avoid pulling non-core logic into default builds.
- `simd-experimental`: reserved for the archived SIMD codec prototype; see `src/storage/engines/core/ops/proximacodec/archive/README.md`.

Default builds keep all of the above off to minimize binary size and surface area. Enable features explicitly when you want to exercise the dormant modules.
