#!/usr/bin/env python3
"""Hard-failing live smoke for the ProximaDB MVP trust corridor.

Only Python's standard library is required. The server may be local, in Docker,
or remote; the test exercises the same canonical REST v2 contract in every case.
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any


@dataclass
class StepResult:
    name: str
    method: str
    path: str
    status: int
    latency_ms: float


class SmokeFailure(RuntimeError):
    pass


class Client:
    def __init__(self, base_url: str, tenant: str | None, timeout: float) -> None:
        self.base_url = base_url.rstrip("/")
        self.tenant = tenant
        self.timeout = timeout
        self.steps: list[StepResult] = []

    def request(
        self,
        name: str,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        expected: tuple[int, ...] = (200, 201, 202, 204),
    ) -> Any:
        headers = {"Accept": "application/json"}
        payload = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            payload = json.dumps(body).encode("utf-8")
        if self.tenant:
            headers["X-Tenant-ID"] = self.tenant
        request = urllib.request.Request(
            f"{self.base_url}{path}", data=payload, headers=headers, method=method
        )
        started = time.perf_counter()
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as exc:
            status = exc.code
            raw = exc.read()
        except OSError as exc:
            raise SmokeFailure(f"{name}: cannot reach {self.base_url}: {exc}") from exc
        latency_ms = (time.perf_counter() - started) * 1000.0
        self.steps.append(StepResult(name, method, path, status, latency_ms))
        text = raw.decode("utf-8", errors="replace")
        if status not in expected:
            raise SmokeFailure(
                f"{name}: {method} {path} returned HTTP {status}; body={text[:1000]}"
            )
        if not text.strip():
            return None
        try:
            return json.loads(text)
        except json.JSONDecodeError as exc:
            raise SmokeFailure(f"{name}: response is not JSON: {text[:1000]}") from exc


def _records(payload: Any) -> list[dict[str, Any]]:
    if not isinstance(payload, dict):
        return []
    for key in ("matches", "results", "records"):
        value = payload.get(key)
        if isinstance(value, list):
            return [item for item in value if isinstance(item, dict)]
    return []


def _record_ids(payload: Any) -> set[str]:
    ids: set[str] = set()
    for record in _records(payload):
        value = record.get("id") or record.get("record_id")
        if isinstance(value, str):
            ids.add(value)
        nested = record.get("record")
        if isinstance(nested, dict) and isinstance(nested.get("id"), str):
            ids.add(nested["id"])
    return ids


def _single_record_id(payload: Any) -> str | None:
    if not isinstance(payload, dict):
        return None
    if isinstance(payload.get("id"), str):
        return payload["id"]
    for key in ("record", "data"):
        nested = payload.get(key)
        if isinstance(nested, dict) and isinstance(nested.get("id"), str):
            return nested["id"]
    return None


def _retry(assertion, timeout: float = 5.0) -> Any:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            return assertion()
        except SmokeFailure as exc:
            last_error = exc
            time.sleep(0.2)
    raise last_error or SmokeFailure("retry deadline expired")


def run_smoke(base_url: str, tenant: str | None = None, timeout: float = 15.0) -> dict:
    client = Client(base_url, tenant, timeout)
    suffix = uuid.uuid4().hex[:12]
    collection = f"mvp_context_{suffix}"
    dimension = 4
    expected_team_ids = {"runbook-api", "incident-api"}

    client.request("health", "GET", "/health")
    client.request(
        "create_collection",
        "POST",
        "/api/v2/collections",
        {
            "name": collection,
            "dimension": dimension,
            "engine": "sst",
            "distance_metric": "cosine",
            "canonical_embedding_precision": "fp32",
            "enable_proxima_record": True,
            "schema": {
                "columns": [
                    {
                        "name": "team",
                        "data_type": "text",
                        "indexed": True,
                        "filterable": True,
                    },
                    {
                        "name": "kind",
                        "data_type": "text",
                        "indexed": True,
                        "filterable": True,
                    },
                ],
                "enforcement": "hybrid",
                "allow_additional_fields": True,
            },
        },
    )
    records = [
        {
            "id": "runbook-api",
            "vector": [1.0, 0.0, 0.0, 0.0],
            "props": {"team": "api", "kind": "runbook", "text": "API recovery"},
        },
        {
            "id": "incident-api",
            "vector": [0.98, 0.02, 0.0, 0.0],
            "props": {"team": "api", "kind": "incident", "text": "API timeout"},
        },
        {
            "id": "runbook-data",
            "vector": [0.0, 1.0, 0.0, 0.0],
            "props": {"team": "data", "kind": "runbook", "text": "Pipeline recovery"},
        },
    ]
    client.request(
        "batch_write",
        "POST",
        f"/api/v2/collections/{collection}/records/batch",
        {"records": records, "upsert": True},
    )

    def filtered_scan() -> Any:
        payload = client.request(
            "filtered_scan",
            "POST",
            f"/api/v2/collections/{collection}/records/scan",
            {"limit": 10, "filter": {"team": "api"}},
        )
        ids = _record_ids(payload)
        if ids != expected_team_ids:
            raise SmokeFailure(f"filtered_scan: expected {expected_team_ids}, received {ids}")
        return payload

    _retry(filtered_scan, timeout=min(timeout, 5.0))

    def filtered_search() -> Any:
        payload = client.request(
            "filtered_search",
            "POST",
            f"/api/v2/collections/{collection}/search",
            {
                "vector": [1.0, 0.0, 0.0, 0.0],
                "top_k": 10,
                "filters": [{"field": "team", "op": "eq", "value": "api"}],
            },
        )
        ids = _record_ids(payload)
        if "runbook-data" in ids or not expected_team_ids.issubset(ids):
            raise SmokeFailure(
                "filtered_search: metadata filter leaked or under-returned; "
                f"received {ids}"
            )
        return payload

    _retry(filtered_search, timeout=min(timeout, 5.0))
    fetched = client.request(
        "get_record",
        "GET",
        f"/api/v2/collections/{collection}/records/runbook-api",
    )
    if _single_record_id(fetched) != "runbook-api":
        raise SmokeFailure("get_record: response did not contain the requested record id")
    route_health = client.request(
        "route_health",
        "GET",
        f"/api/v2/_diagnostics/collections/{collection}/route-health",
    )
    required_health = {
        "writes",
        "freshness",
        "filtered_ann",
        "object_economy",
        "recall_probe",
    }
    missing = required_health.difference(route_health if isinstance(route_health, dict) else {})
    if missing:
        raise SmokeFailure(f"route_health: missing diagnostic blocks {sorted(missing)}")
    writes = route_health.get("writes", {})
    for unsupported in ("conditional_write", "filter_write", "patch"):
        if writes.get(unsupported) is not False:
            raise SmokeFailure(f"route_health: writes.{unsupported} must be false for MVP")

    client.request(
        "delete_record",
        "DELETE",
        f"/api/v2/collections/{collection}/records/runbook-api",
    )

    def post_delete_search() -> Any:
        payload = client.request(
            "post_delete_search",
            "POST",
            f"/api/v2/collections/{collection}/search",
            {
                "vector": [1.0, 0.0, 0.0, 0.0],
                "top_k": 10,
                "filters": [{"field": "team", "op": "eq", "value": "api"}],
            },
        )
        if "runbook-api" in _record_ids(payload):
            raise SmokeFailure("post_delete_search: tombstoned record is still visible")
        return payload

    _retry(post_delete_search, timeout=min(timeout, 5.0))

    latencies = [step.latency_ms for step in client.steps]
    return {
        "schema_version": 1,
        "status": "passed",
        "base_url": client.base_url,
        "collection": collection,
        "canonical_api": "REST /api/v2",
        "engine": "sst",
        "step_count": len(client.steps),
        "latency_ms": {
            "median": round(statistics.median(latencies), 3),
            "max": round(max(latencies), 3),
        },
        "steps": [asdict(step) for step in client.steps],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", default="http://127.0.0.1:5678")
    parser.add_argument("--tenant", default=None)
    parser.add_argument("--timeout", type=float, default=15.0)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        report = run_smoke(args.base_url, args.tenant, args.timeout)
    except SmokeFailure as exc:
        print(f"MVP smoke FAILED: {exc}", file=sys.stderr)
        return 1
    rendered = json.dumps(report, indent=2, sort_keys=True)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered + "\n", encoding="utf-8")
    print(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
