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
    "proximadb-graph-subset",
}
QUERY_RUNTIME_DISALLOWED_CONTRACTS = QUERY_ADAPTER_CRATES


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
    if parts[:2] == ("crates", "modalities"):
        return "modality"
    if parts[:2] == ("crates", "query"):
        if name in QUERY_ADAPTER_CRATES:
            return "query-adapter"
        return "query-runtime" if name in QUERY_RUNTIME_CRATES else "query-contract"
    if parts and parts[0] == "clients":
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
            if crate.layer == "foundation" and dep.layer != "foundation":
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep.name,
                        "foundation crates may only depend on other foundation crates",
                    )
                )

            if crate.layer == "modality" and dep.layer in {"root", "application", "binding"}:
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep.name,
                        "modality crates must not depend on root/application/binding crates",
                    )
                )

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

            if crate.layer == "query-contract" and dep.layer == "query-runtime":
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep.name,
                        "query contract crates must not depend on query runtime crates",
                    )
                )

            if crate.layer == "query-contract" and dep.layer == "query-adapter":
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep.name,
                        "query contract crates must not depend on query adapter/runtime crates",
                    )
                )

            if crate.layer.startswith("query") and dep.layer in {
                "root",
                "application",
                "binding",
            }:
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep.name,
                        "query crates must not depend on root/application/binding crates",
                    )
                )

            if crate.layer in {"query-contract", "query-adapter"} and dep.layer == "modality":
                findings.append(
                    Finding(
                        "warning",
                        crate.name,
                        dep.name,
                        "query contracts still depend on modality runtime; migrate to narrower contracts",
                    )
                )

            if crate.layer == "query-runtime" and dep.layer == "modality":
                findings.append(
                    Finding(
                        "error",
                        crate.name,
                        dep.name,
                        "query runtime crates must depend on modality contracts/capabilities, not concrete modality runtimes",
                    )
                )

            if (
                crate.layer == "query-runtime"
                and dep.name in QUERY_RUNTIME_DISALLOWED_CONTRACTS
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--strict",
        action="store_true",
        help="treat transitional warnings as failures",
    )
    args = parser.parse_args()

    crates = workspace_crates()
    findings = check_boundaries(crates)
    print_report(crates, findings)

    has_errors = any(f.severity == "error" for f in findings)
    has_warnings = any(f.severity == "warning" for f in findings)
    if has_errors or (args.strict and has_warnings):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
