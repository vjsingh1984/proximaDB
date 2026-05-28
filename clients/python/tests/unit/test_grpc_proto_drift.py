"""gRPC proto / Python-stub drift gate.

For every ``.proto`` file in ``proto/proximadb/**`` that declares one or
more services, this test computes the drift set:

  * ``missing-stub:<proto-path>`` — no matching ``*_pb2_grpc.py`` exists.
  * ``missing-servicer:<proto>::<Service>`` — servicer class not generated.
  * ``missing-rpc:<proto>::<Service>.<Rpc>`` — rpc not present on its servicer.

The gate compares the live drift set against
``tests/unit/grpc_drift_baseline.json``:

  * Fails on NEW drift (i.e. drift not in the baseline) — any new proto
    service or rpc must come with a regenerated Python stub.
  * Fails on baseline entries that no longer drift — when stubs are
    regenerated, the baseline must shrink to match.

This makes the gate ratcheting: existing technical debt is accepted, but
*new* drift is blocked and *resolved* drift forces baseline maintenance.
"""

from __future__ import annotations

import ast
import json
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[4]
PROTO_ROOT = REPO_ROOT / "proto" / "proximadb"
PYTHON_SRC_ROOT = REPO_ROOT / "clients" / "python" / "src"
BASELINE_PATH = Path(__file__).resolve().parent / "grpc_drift_baseline.json"


# ---------------------------------------------------------------------------
# Proto parsing (regex-based, sufficient for service / rpc surface)
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class ProtoService:
    name: str
    rpcs: tuple[str, ...]


@dataclass(frozen=True)
class ProtoFile:
    path: Path
    package: str
    services: tuple[ProtoService, ...]


_SERVICE_RE = re.compile(r"^\s*service\s+(\w+)\s*\{", re.MULTILINE)
_RPC_RE = re.compile(r"^\s*rpc\s+(\w+)\s*\(", re.MULTILINE)
_PACKAGE_RE = re.compile(r"^\s*package\s+([\w\.]+)\s*;", re.MULTILINE)


def _parse_proto(path: Path) -> ProtoFile:
    text = path.read_text()
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    text = re.sub(r"//[^\n]*", "", text)

    package_match = _PACKAGE_RE.search(text)
    package = package_match.group(1) if package_match else ""

    services: list[ProtoService] = []
    for svc_match in _SERVICE_RE.finditer(text):
        svc_start = svc_match.end()
        depth = 1
        i = svc_start
        while i < len(text) and depth > 0:
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
            i += 1
        svc_body = text[svc_start : i - 1]
        rpcs = tuple(m.group(1) for m in _RPC_RE.finditer(svc_body))
        services.append(ProtoService(name=svc_match.group(1), rpcs=rpcs))

    return ProtoFile(path=path, package=package, services=tuple(services))


# ---------------------------------------------------------------------------
# Generated-stub indexing
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class StubModule:
    path: Path
    servicers: dict[str, set[str]]  # ServicerClassName -> {method, ...}


def _parse_generated_stub(path: Path) -> StubModule:
    tree = ast.parse(path.read_text())
    servicers: dict[str, set[str]] = {}
    for node in tree.body:
        if isinstance(node, ast.ClassDef) and node.name.endswith("Servicer"):
            methods: set[str] = set()
            for sub in node.body:
                if isinstance(sub, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    methods.add(sub.name)
            servicers[node.name] = methods
    return StubModule(path=path, servicers=servicers)


# ---------------------------------------------------------------------------
# Drift computation
# ---------------------------------------------------------------------------


def _rel(p: Path) -> str:
    return str(p.relative_to(REPO_ROOT))


def _compute_drift() -> set[str]:
    """Return the current drift set in stable string form."""
    proto_files = [_parse_proto(p) for p in sorted(PROTO_ROOT.rglob("*.proto"))]
    stubs = [
        _parse_generated_stub(p) for p in sorted(PYTHON_SRC_ROOT.rglob("*_pb2_grpc.py"))
    ]
    stubs_by_basename: dict[str, list[StubModule]] = defaultdict(list)
    for s in stubs:
        stubs_by_basename[s.path.name].append(s)
    all_servicers: dict[str, set[str]] = defaultdict(set)
    for stub in stubs:
        for class_name, methods in stub.servicers.items():
            all_servicers[class_name] |= methods

    drift: set[str] = set()
    for proto in proto_files:
        if not proto.services:
            continue
        expected_basename = proto.path.stem + "_pb2_grpc.py"
        if expected_basename not in stubs_by_basename:
            drift.add(f"missing-stub:{_rel(proto.path)}")

        for service in proto.services:
            servicer = f"{service.name}Servicer"
            if servicer not in all_servicers:
                drift.add(f"missing-servicer:{_rel(proto.path)}::{service.name}")
                continue
            for rpc in service.rpcs:
                if rpc not in all_servicers[servicer]:
                    drift.add(f"missing-rpc:{_rel(proto.path)}::{service.name}.{rpc}")
    return drift


def _load_baseline() -> set[str]:
    if not BASELINE_PATH.exists():
        return set()
    data = json.loads(BASELINE_PATH.read_text())
    return set(data.get("accepted_drift", []))


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture(scope="module")
def proto_files() -> list[ProtoFile]:
    return [_parse_proto(p) for p in sorted(PROTO_ROOT.rglob("*.proto"))]


@pytest.fixture(scope="module")
def current_drift() -> set[str]:
    return _compute_drift()


@pytest.fixture(scope="module")
def baseline() -> set[str]:
    return _load_baseline()


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_proto_inventory_nonempty(proto_files):
    """Sanity: parsing finds protos and at least one declares a service."""
    assert proto_files, "No .proto files found under proto/proximadb"
    assert any(
        p.services for p in proto_files
    ), "No .proto files declare any service — parser is likely broken"


def test_no_new_proto_stub_drift(current_drift, baseline):
    """New proto services / rpcs must come with regenerated Python stubs."""
    new_drift = current_drift - baseline
    assert not new_drift, (
        "New proto/stub drift introduced (not in baseline):\n  - "
        + "\n  - ".join(sorted(new_drift))
        + f"\n\nEither regenerate Python gRPC stubs for the affected protos, "
        f"or — if the drift is known and accepted — add the entries to:\n  {BASELINE_PATH}"
    )


def test_baseline_does_not_contain_resolved_drift(current_drift, baseline):
    """When stubs are regenerated, the baseline must shrink to match."""
    resolved = baseline - current_drift
    assert not resolved, (
        "Baseline lists drift that no longer exists — please remove these "
        "entries from the baseline manifest:\n  - "
        + "\n  - ".join(sorted(resolved))
        + f"\n\nBaseline: {BASELINE_PATH}"
    )
