//! Spec-from-code generator + drift gate for the published REST contract
//! (TD-126 Phase 1).
//!
//! `docs/openapi/proximadb-openapi.yaml` is GENERATED from the annotated axum
//! handlers (see `src/network/rest/openapi.rs`), not hand-maintained. This test
//! is both the generator and the drift gate:
//!
//!   * `UPDATE_OPENAPI_SPEC=1 cargo test --test openapi_spec_gen` regenerates
//!     and writes the committed YAML (run this after changing a handler/DTO).
//!   * `cargo test --test openapi_spec_gen` (the default, and what CI runs)
//!     regenerates in memory and FAILS if it differs from the committed copy.
//!
//! Mirrors the `proto-compat` generated-artifact gate: the spec can never
//! silently diverge from the handler code.

use std::path::PathBuf;

use proximadb::network::rest::openapi::openapi_yaml;

fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/openapi/proximadb-openapi.yaml")
}

#[test]
fn openapi_spec_matches_handlers() {
    let generated = openapi_yaml().expect("generate OpenAPI document from handlers");
    let path = spec_path();

    if std::env::var_os("UPDATE_OPENAPI_SPEC").is_some() {
        std::fs::write(&path, generated.as_bytes())
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        eprintln!("Wrote regenerated OpenAPI spec to {}", path.display());
        return;
    }

    let committed = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read committed OpenAPI spec at {}: {e}\n\
             Run `UPDATE_OPENAPI_SPEC=1 cargo test --test openapi_spec_gen` to generate it.",
            path.display()
        )
    });

    assert!(
        committed == generated,
        "docs/openapi/proximadb-openapi.yaml is out of sync with the annotated REST handlers.\n\
         The OpenAPI spec is generated from code (TD-126); do not hand-edit it.\n\
         Run `UPDATE_OPENAPI_SPEC=1 cargo test --test openapi_spec_gen` and commit the result."
    );
}
