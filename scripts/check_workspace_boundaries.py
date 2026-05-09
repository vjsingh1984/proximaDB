#!/usr/bin/env python3
"""Validate ProximaDB workspace dependency boundaries.

This is intentionally lightweight: it reads Cargo manifests directly so it can
run before or during expensive Rust builds without taking a cargo build lock.
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parents[1]


FOUNDATION_ALLOWED_TRANSPORT = {"proximadb-proto"}
FOUNDATION_DISALLOWED_DEPS: dict[str, set[str]] = {
    "proximadb-kernel": {"tonic", "tonic-prost"},
    "proximadb-proto": {"num_cpus", "sysinfo", "tracing"},
}
MODALITY_ALLOWED_QUERY_CONTRACTS = {"proximadb-graph-query", "proximadb-query-filter"}
QUERY_RUNTIME_CRATES = {"proximadb-query"}
QUERY_ADAPTER_CRATES = {
    "proximadb-graph-arrow",
    "proximadb-graph-subset",
    "proximadb-uql",
}
QUERY_PLANNER_CRATES = {
    "proximadb-multimodel-plan",
}
QUERY_RUNTIME_DISALLOWED_ADAPTERS = {
    "proximadb-graph-arrow",
    "proximadb-graph-subset",
}
QUERY_LAYERS = frozenset(
    {"query-contract", "query-planner", "query-adapter", "query-runtime"}
)
APPLICATION_LAYERS = frozenset({"root", "application", "binding"})
ALL_LAYERS = frozenset(
    {
        "foundation",
        "storage",
        "horizontal",
        "modality",
        "platform",
        "integration",
        *QUERY_LAYERS,
        *APPLICATION_LAYERS,
    }
)
NON_FOUNDATION_LAYERS = ALL_LAYERS - {"foundation"}


@dataclass(frozen=True)
class Crate:
    name: str
    path: Path
    layer: str
    deps: tuple[str, ...]


@dataclass(frozen=True)
class Finding:
    severity: str
    crate: str
    dependency: str
    message: str


@dataclass(frozen=True)
class LayerRule:
    source_layers: frozenset[str]
    dependency_layers: frozenset[str]
    severity: str
    message: str

    def applies_to(self, crate: Crate, dependency: Crate) -> bool:
        return (
            crate.layer in self.source_layers
            and dependency.layer in self.dependency_layers
        )


@dataclass(frozen=True)
class SourceModuleTarget:
    layer: str
    target: str
    rationale: str


LAYER_RULES = (
    LayerRule(
        frozenset({"foundation"}),
        NON_FOUNDATION_LAYERS,
        "error",
        "foundation crates may only depend on other foundation crates",
    ),
    LayerRule(
        frozenset({"storage"}),
        frozenset(
            {
                "modality",
                "platform",
                "integration",
                *QUERY_LAYERS,
                *APPLICATION_LAYERS,
            }
        ),
        "error",
        "storage-common crates must not depend upward into modality, query, platform, integration, or app layers",
    ),
    LayerRule(
        frozenset({"horizontal"}),
        frozenset(
            {
                "storage",
                "modality",
                "platform",
                "integration",
                *QUERY_LAYERS,
                *APPLICATION_LAYERS,
            }
        ),
        "error",
        "horizontal infrastructure crates must stay reusable and not depend on domain, platform, integration, or app layers",
    ),
    LayerRule(
        frozenset({"modality"}),
        frozenset({"platform", "integration", *APPLICATION_LAYERS}),
        "error",
        "modality crates must not depend on platform/integration/root/application/binding crates",
    ),
    LayerRule(
        frozenset({"query-contract"}),
        frozenset({"query-runtime"}),
        "error",
        "query contract crates must not depend on query runtime crates",
    ),
    LayerRule(
        frozenset({"query-contract"}),
        frozenset({"query-adapter"}),
        "error",
        "query contract crates must not depend on query adapter/runtime crates",
    ),
    LayerRule(
        frozenset({"query-contract"}),
        frozenset({"query-planner"}),
        "error",
        "query contract crates must not depend on planner/optimizer behavior crates",
    ),
    LayerRule(
        QUERY_LAYERS,
        frozenset({"platform", "integration", *APPLICATION_LAYERS}),
        "error",
        "query crates must not depend on platform/integration/root/application/binding crates",
    ),
    LayerRule(
        frozenset({"query-contract", "query-adapter"}),
        frozenset({"modality"}),
        "warning",
        "query contract/adapter crates still depend on modality runtime; migrate to narrower contracts",
    ),
    LayerRule(
        frozenset({"query-planner"}),
        frozenset({"query-runtime", "query-adapter", "modality"}),
        "error",
        "query planner crates must depend on contracts/foundation, not runtime, adapter, or modality crates",
    ),
    LayerRule(
        frozenset({"query-runtime"}),
        frozenset({"modality"}),
        "error",
        "query runtime crates must depend on modality contracts/capabilities, not concrete modality runtimes",
    ),
    LayerRule(
        frozenset({"integration"}),
        frozenset(
            {
                "modality",
                "platform",
                "query-adapter",
                "query-planner",
                "query-runtime",
                *APPLICATION_LAYERS,
            }
        ),
        "error",
        "integration crates must terminate at foundation, horizontal, and stable contract layers",
    ),
    LayerRule(
        frozenset({"platform"}),
        APPLICATION_LAYERS,
        "error",
        "platform/runtime crates must not depend on root/application/binding crates",
    ),
)

SRC_MODULE_TARGETS: dict[str, SourceModuleTarget] = {
    "ai": SourceModuleTarget(
        "integration",
        "proximadb-integrations",
        "provider/model adapters should terminate at stable query/runtime contracts",
    ),
    "analytics": SourceModuleTarget(
        "application",
        "apps/proximadb-bench or future analytics app crate",
        "product analytics should not become shared runtime infrastructure",
    ),
    "api_handlers": SourceModuleTarget(
        "platform",
        "proximadb-api",
        "REST/gRPC/Arrow/pgwire request handling belongs above query/runtime contracts",
    ),
    "audit": SourceModuleTarget(
        "horizontal",
        "proximadb-security or proximadb-telemetry",
        "audit is shared policy/observability infrastructure, not modality logic",
    ),
    "auth": SourceModuleTarget(
        "horizontal",
        "proximadb-security",
        "authentication is shared runtime policy infrastructure",
    ),
    "automl": SourceModuleTarget(
        "integration",
        "proximadb-integrations",
        "AutoML/provider integration should adapt to stable query/vector contracts",
    ),
    "bench": SourceModuleTarget(
        "application",
        "apps/proximadb-bench",
        "benchmarks should depend on public runtime surfaces, not internal modules",
    ),
    "bin": SourceModuleTarget(
        "application",
        "apps/proximadb-server and apps/proximadb-bench",
        "binaries should become thin composition entrypoints",
    ),
    "catalog": SourceModuleTarget(
        "platform",
        "proximadb-runtime or future proximadb-catalog",
        "catalog/control-plane ownership sits above modality storage contracts",
    ),
    "cdc": SourceModuleTarget(
        "integration",
        "proximadb-integrations",
        "CDC connectors adapt external streams to stable internal contracts",
    ),
    "cluster": SourceModuleTarget(
        "horizontal",
        "proximadb-distributed",
        "cluster membership, placement, and consensus are shared distributed infrastructure",
    ),
    "compute": SourceModuleTarget(
        "horizontal",
        "proximadb-runtime-common or proximadb-vector",
        "shared execution helpers stay horizontal; vector-specific kernels move to vector",
    ),
    "config": SourceModuleTarget(
        "foundation",
        "proximadb-config",
        "validated configuration models should not live in the root runtime monolith",
    ),
    "connectors": SourceModuleTarget(
        "integration",
        "proximadb-integrations",
        "external connectors adapt outside systems to internal contracts",
    ),
    "core": SourceModuleTarget(
        "foundation",
        "proximadb-kernel, proximadb-data-model, proximadb-records",
        "core primitives should be split into narrow foundation crates",
    ),
    "datafusion": SourceModuleTarget(
        "integration",
        "proximadb-integrations or proximadb-query adapter",
        "DataFusion integration is an adapter over query/runtime contracts",
    ),
    "deployment": SourceModuleTarget(
        "application",
        "deploy/packaging or apps support crates",
        "deployment automation should stay outside core product runtime",
    ),
    "embedded": SourceModuleTarget(
        "platform",
        "proximadb-runtime or language embedded binding crates",
        "embedded composition belongs at runtime/binding boundaries",
    ),
    "errors": SourceModuleTarget(
        "foundation",
        "proximadb-kernel",
        "shared error/result contracts belong in the kernel foundation crate",
    ),
    "executive": SourceModuleTarget(
        "application",
        "future enterprise/application crate",
        "executive/business workflows should not become shared infrastructure",
    ),
    "graph": SourceModuleTarget(
        "modality",
        "proximadb-graph",
        "graph engines, traversal, and graph query runtime belong to the graph modality",
    ),
    "index": SourceModuleTarget(
        "modality",
        "proximadb-vector or proximadb-storage-common",
        "vector indexes move to vector; truly shared index abstractions move lower",
    ),
    "infrastructure": SourceModuleTarget(
        "horizontal",
        "proximadb-runtime-common",
        "shared pools, schedulers, and runtime helpers should be explicit horizontal infrastructure",
    ),
    "licensing": SourceModuleTarget(
        "platform",
        "proximadb-runtime",
        "license enforcement composes with runtime policy, not foundation contracts",
    ),
    "llm": SourceModuleTarget(
        "integration",
        "proximadb-integrations",
        "LLM/provider adapters should remain outside core query/runtime behavior",
    ),
    "metrics": SourceModuleTarget(
        "horizontal",
        "proximadb-telemetry",
        "metrics contracts and helpers are shared telemetry infrastructure",
    ),
    "monitoring": SourceModuleTarget(
        "horizontal",
        "proximadb-telemetry",
        "monitoring/reporting belongs with telemetry/runtime observability infrastructure",
    ),
    "network": SourceModuleTarget(
        "horizontal",
        "proximadb-network",
        "protocol-neutral networking, sessions, middleware, and transport helpers are horizontal",
    ),
    "observability": SourceModuleTarget(
        "modality",
        "proximadb-observability",
        "logs, metrics, traces, and event-query runtime form an observability modality",
    ),
    "operations": SourceModuleTarget(
        "platform",
        "proximadb-runtime",
        "backup/restore/admin operations compose runtime services",
    ),
    "prompts": SourceModuleTarget(
        "integration",
        "proximadb-integrations",
        "prompt templates support LLM/provider adapters, not core runtime contracts",
    ),
    "proto": SourceModuleTarget(
        "foundation",
        "proximadb-proto",
        "generated protocol contracts stay in the protocol foundation crate",
    ),
    "query": SourceModuleTarget(
        "query",
        "proximadb-query plus query contract/adapter crates",
        "cross-model planning, lowering, routing, and fusion belong to query strata",
    ),
    "revenue": SourceModuleTarget(
        "application",
        "future business/application crate",
        "revenue workflows should remain outside core database runtime layers",
    ),
    "sales_enablement": SourceModuleTarget(
        "application",
        "future demo/application crate",
        "demo/sales workflows should depend on product APIs, not internal modules",
    ),
    "schema": SourceModuleTarget(
        "foundation",
        "proximadb-data-model or modality-specific schema crates",
        "common schema/value contracts move lower; modality-only schema stays with modality",
    ),
    "search": SourceModuleTarget(
        "modality",
        "proximadb-vector or proximadb-query",
        "vector search runtime belongs to vector; cross-model search orchestration belongs to query",
    ),
    "security": SourceModuleTarget(
        "horizontal",
        "proximadb-security",
        "authorization, policy, crypto, and RLS are shared security infrastructure",
    ),
    "server": SourceModuleTarget(
        "application",
        "apps/proximadb-server",
        "server startup should be a thin runtime composition entrypoint",
    ),
    "services": SourceModuleTarget(
        "platform",
        "proximadb-runtime or modality crates",
        "service wiring moves to runtime; domain services move to their owning modality",
    ),
    "storage": SourceModuleTarget(
        "storage",
        "proximadb-storage-common plus modality crates",
        "common storage primitives move lower; modality storage stays with owning modality",
    ),
    "streaming": SourceModuleTarget(
        "integration",
        "proximadb-integrations",
        "streaming adapters connect external systems to internal ingestion/query contracts",
    ),
    "transaction": SourceModuleTarget(
        "horizontal",
        "proximadb-runtime-common or future proximadb-transaction",
        "transaction coordination is shared runtime infrastructure",
    ),
    "utils": SourceModuleTarget(
        "foundation",
        "specific foundation or runtime-common crates",
        "generic utilities must be assigned to the narrowest owning layer",
    ),
    "vector": SourceModuleTarget(
        "modality",
        "proximadb-vector",
        "vector-specific runtime, scoring, and indexing belong to the vector modality",
    ),
}


def load_toml(path: Path) -> dict:
    with path.open("rb") as handle:
        return tomllib.load(handle)


def dependency_names(manifest: dict) -> tuple[str, ...]:
    names: set[str] = set()
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        for name in manifest.get(section, {}):
            names.add(name)

    for target in manifest.get("target", {}).values():
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for name in target.get(section, {}):
                names.add(name)

    return tuple(sorted(names))


def crate_layer(member_path: Path, name: str) -> str:
    parts = member_path.parts
    if member_path == Path("."):
        return "root"
    if parts[:2] == ("crates", "foundation"):
        return "foundation"
    if parts[:2] == ("crates", "storage"):
        return "storage"
    if parts[:2] == ("crates", "horizontal"):
        return "horizontal"
    if parts[:2] == ("crates", "modalities"):
        return "modality"
    if parts[:2] == ("crates", "query"):
        if name in QUERY_ADAPTER_CRATES:
            return "query-adapter"
        if name in QUERY_PLANNER_CRATES:
            return "query-planner"
        return "query-runtime" if name in QUERY_RUNTIME_CRATES else "query-contract"
    if parts[:2] == ("crates", "platform"):
        return "platform"
    if parts[:2] == ("crates", "integrations"):
        return "integration"
    if parts and parts[0] == "apps":
        return "application"
    if parts and parts[0] in {"bindings", "clients"}:
        return "binding"
    return "application"


def workspace_crates() -> dict[str, Crate]:
    root_manifest = load_toml(ROOT / "Cargo.toml")
    members = root_manifest.get("workspace", {}).get("members", [])
    crates: dict[str, Crate] = {}

    for member in members:
        member_path = Path(member)
        manifest_path = ROOT / member_path / "Cargo.toml"
        manifest = load_toml(manifest_path)
        package = manifest.get("package", {})
        name = package.get("name")
        if not name:
            continue
        crates[name] = Crate(
            name=name,
            path=member_path,
            layer=crate_layer(member_path, name),
            deps=dependency_names(manifest),
        )

    return crates


def internal_deps(crate: Crate, crates: dict[str, Crate]) -> Iterable[Crate]:
    for dep_name in crate.deps:
        dep = crates.get(dep_name)
        if dep is not None:
            yield dep


def check_boundaries(crates: dict[str, Crate]) -> list[Finding]:
    findings: list[Finding] = []

    for crate in crates.values():
        for dep_name in FOUNDATION_DISALLOWED_DEPS.get(crate.name, set()):
            if dep_name in crate.deps:
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep_name,
                        "foundation crate carries a disallowed transport/runtime helper dependency",
                    )
                )

        for dep in internal_deps(crate, crates):
            if (
                crate.layer == "modality"
                and dep.layer.startswith("query")
                and dep.name not in MODALITY_ALLOWED_QUERY_CONTRACTS
            ):
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep.name,
                        "modality crates may only depend on approved query contract crates",
                    )
                )

            for rule in LAYER_RULES:
                if rule.applies_to(crate, dep):
                    findings.append(
                        Finding(
                            rule.severity,
                            crate.name,
                            dep.name,
                            rule.message,
                        )
                    )

            if (
                crate.layer == "query-runtime"
                and dep.name in QUERY_RUNTIME_DISALLOWED_ADAPTERS
            ):
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep.name,
                        "query runtime crates must not depend on adapter/runtime-flavored query contract crates",
                    )
                )

    return findings


def print_report(crates: dict[str, Crate], findings: list[Finding]) -> None:
    print("Workspace boundary check")
    print(f"workspace crates: {len(crates)}")

    by_layer: dict[str, list[str]] = {}
    for crate in crates.values():
        by_layer.setdefault(crate.layer, []).append(crate.name)

    for layer in sorted(by_layer):
        names = ", ".join(sorted(by_layer[layer]))
        print(f"{layer}: {names}")

    if findings:
        print()
        print("Findings:")
        for finding in findings:
            print(
                f"{finding.severity.upper()}: {finding.crate} -> "
                f"{finding.dependency}: {finding.message}"
            )
    else:
        print()
        print("No boundary findings.")


def print_rules() -> None:
    print("Workspace boundary rules")
    print()
    print("Layer rules:")
    for rule in LAYER_RULES:
        sources = ", ".join(sorted(rule.source_layers))
        dependencies = ", ".join(sorted(rule.dependency_layers))
        print(f"{rule.severity.upper()}: {sources} -> {dependencies}: {rule.message}")

    print()
    print("Crate-specific rules:")
    for crate, dependency_names in sorted(FOUNDATION_DISALLOWED_DEPS.items()):
        dependencies = ", ".join(sorted(dependency_names))
        print(
            f"ERROR: {crate} must not depend on {dependencies}: "
            "foundation crate carries a disallowed transport/runtime helper dependency"
        )

    contracts = ", ".join(sorted(MODALITY_ALLOWED_QUERY_CONTRACTS))
    print(
        "ERROR: modality -> query-* is forbidden except "
        f"{contracts}: modality crates may only depend on approved query contract crates"
    )

    adapters = ", ".join(sorted(QUERY_RUNTIME_DISALLOWED_ADAPTERS))
    print(
        f"ERROR: query-runtime -> {adapters} is forbidden: "
        "query runtime crates must not depend on adapter/runtime-flavored query contract crates"
    )


def print_dependency_map(crates: dict[str, Crate]) -> None:
    print("Workspace dependency map")
    print(f"workspace crates: {len(crates)}")
    print()

    for crate in sorted(crates.values(), key=lambda item: (item.layer, item.name)):
        dependencies = sorted(internal_deps(crate, crates), key=lambda item: item.name)
        if dependencies:
            rendered = ", ".join(
                f"{dependency.name} [{dependency.layer}]"
                for dependency in dependencies
            )
        else:
            rendered = "(no internal workspace dependencies)"
        print(f"{crate.name} [{crate.layer}] -> {rendered}")


def source_modules() -> tuple[str, ...]:
    src_dir = ROOT / "src"
    return tuple(
        sorted(path.name for path in src_dir.iterdir() if path.is_dir() and path.name != "tests")
    )


def print_source_module_map() -> int:
    print("Root source module migration map")
    modules = source_modules()
    print(f"top-level src modules: {len(modules)}")
    print()

    unmapped: list[str] = []
    for module in modules:
        target = SRC_MODULE_TARGETS.get(module)
        if target is None:
            unmapped.append(module)
            print(f"src/{module} -> UNMAPPED [classify before extracting]")
            continue

        print(f"src/{module} -> {target.target} [{target.layer}]")
        print(f"  {target.rationale}")

    if unmapped:
        print()
        print("Unmapped modules:")
        for module in unmapped:
            print(f"src/{module}")
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--strict",
        action="store_true",
        help="treat transitional warnings as failures",
    )
    parser.add_argument(
        "--rules",
        action="store_true",
        help="print the enforced boundary policy and exit",
    )
    parser.add_argument(
        "--deps",
        action="store_true",
        help="print the workspace crate dependency map and exit",
    )
    parser.add_argument(
        "--src-map",
        action="store_true",
        help="print the root src module migration map and exit",
    )
    args = parser.parse_args()

    if args.rules:
        print_rules()
        return 0

    if args.src_map:
        return print_source_module_map()

    crates = workspace_crates()
    if args.deps:
        print_dependency_map(crates)
        return 0

    findings = check_boundaries(crates)
    print_report(crates, findings)

    has_errors = any(f.severity == "error" for f in findings)
    has_warnings = any(f.severity == "warning" for f in findings)
    if has_errors or (args.strict and has_warnings):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
