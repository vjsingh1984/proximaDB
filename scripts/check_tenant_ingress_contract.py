#!/usr/bin/env python3
"""Guard deployment-aware tenant resolution at served network ingress."""

from __future__ import annotations

import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]


def read(relative: str) -> str:
    return (REPO / relative).read_text(encoding="utf-8")


def function_body(source: str, function_name: str) -> str:
    marker = f"async fn {function_name}("
    start = source.find(marker)
    if start < 0:
        raise ValueError(f"missing function {function_name}")
    opening = source.find("{", start)
    if opening < 0:
        raise ValueError(f"missing body for {function_name}")

    depth = 0
    for index in range(opening, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[opening : index + 1]
    raise ValueError(f"unterminated body for {function_name}")


def main() -> int:
    errors: list[str] = []

    grpc_files = [
        "src/network/grpc/v2/document_service.rs",
        "src/network/grpc/v2/entity_service.rs",
        "src/network/grpc/v2/fusion_service.rs",
        "src/network/grpc/v2/graph_service.rs",
        "src/network/grpc/v2/record_service.rs",
    ]
    for relative in grpc_files:
        source = read(relative)
        if "grpc_auth::tenant_id(" in source:
            errors.append(f"{relative}: optional grpc_auth::tenant_id bypass remains")
        if "extract_tenant_id" in source:
            errors.append(f"{relative}: local tenant extraction bypass remains")
        if "grpc_auth::resolved_tenant_id(" not in source:
            errors.append(f"{relative}: deployment-aware resolver is not used")

    flight = read("src/network/arrow_ipc/service.rs")
    for method in (
        "list_flights",
        "get_flight_info",
        "get_schema",
        "do_get",
        "do_put",
        "do_action",
        "do_exchange",
    ):
        try:
            body = function_body(flight, method)
        except ValueError as error:
            errors.append(f"src/network/arrow_ipc/service.rs: {error}")
            continue
        if ".authenticated_flight_context(" not in body:
            errors.append(
                f"src/network/arrow_ipc/service.rs: {method} skips tenant/auth resolution"
            )

    if ".handle_vector_search_v1(request)" in flight:
        errors.append("Arrow Flight search uses the tenant-neutral API port")
    if ".upsert_record_batch(collection_id, records, None)" in flight:
        errors.append("Arrow Flight action upsert drops the resolved tenant")

    multi_server = read("src/network/multi_server.rs")
    if multi_server.count("GrpcTenantModeLayer::new(") < 3:
        errors.append("not every gRPC server composition installs GrpcTenantModeLayer")
    if multi_server.count("with_tenant_deployment_mode(") < 5:
        errors.append("not every Flight/pgwire composition receives tenant deployment mode")

    pgwire = read("src/network/postgres/protocol.rs")
    try:
        startup = function_body(pgwire, "handle_startup")
        if "resolve_startup_tenant(" not in startup:
            errors.append("pgwire startup skips deployment-aware tenant resolution")
    except ValueError as error:
        errors.append(f"src/network/postgres/protocol.rs: {error}")

    for error in errors:
        print(f"ERROR {error}", file=sys.stderr)
    if errors:
        print(f"Tenant ingress contract FAILED: {len(errors)} error(s)", file=sys.stderr)
        return 1

    print("Tenant ingress contract OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
