"""Unit tests for scripts/affected_crates.py's workspace-dependent closure.

Guards the touched-crate fast lane's cost invariant: `-p <crate>` compiles that
crate's WHOLE dependency graph, so any workspace member that transitively
depends on the ~850K-LOC root `proximadb` monolith must be excluded from the
lane — naming the root package alone is not enough. Regressing this closure
silently reintroduces the 15-minute-timeout class the lane was fixed for.

Pure-synthetic (no `cargo metadata` invocation) so it runs in the toolchain-free
capability-matrix CI job.
"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "affected_crates", ROOT / "scripts/affected_crates.py"
)
assert SPEC and SPEC.loader
AFFECTED = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(AFFECTED)


def _meta(graph: dict[str, list[str]]) -> dict:
    """Build a minimal `cargo metadata --no-deps`-shaped dict from name -> deps."""
    return {
        "packages": [
            {"name": name, "dependencies": [{"name": d} for d in deps]}
            for name, deps in graph.items()
        ]
    }


class WorkspaceDependentsTest(unittest.TestCase):
    def test_excludes_root_itself(self) -> None:
        meta = _meta({"proximadb": [], "leaf": []})
        self.assertEqual(
            AFFECTED.workspace_dependents_of(meta, ["proximadb"]), {"proximadb"}
        )

    def test_excludes_direct_dependent(self) -> None:
        # The crates/binding/proximadb-embedded shape: a direct path dep on root.
        meta = _meta({"proximadb": [], "embedded": ["proximadb"], "leaf": []})
        self.assertEqual(
            AFFECTED.workspace_dependents_of(meta, ["proximadb"]),
            {"proximadb", "embedded"},
        )

    def test_excludes_transitive_dependent(self) -> None:
        # The clients/rust shape: sdk -> embedded -> proximadb. `sdk` never names
        # the root, but `-p sdk` still drags the whole monolith in.
        meta = _meta(
            {
                "proximadb": [],
                "embedded": ["proximadb"],
                "sdk": ["embedded"],
                "leaf": ["some-third-party"],
            }
        )
        self.assertEqual(
            AFFECTED.workspace_dependents_of(meta, ["proximadb"]),
            {"proximadb", "embedded", "sdk"},
        )

    def test_keeps_unrelated_leaves(self) -> None:
        meta = _meta(
            {
                "proximadb": ["foundation"],
                "foundation": [],
                "codec": ["foundation"],
                "embedded": ["proximadb"],
            }
        )
        excluded = AFFECTED.workspace_dependents_of(meta, ["proximadb"])
        # Crates the monolith depends ON stay eligible — the edge is directional.
        self.assertNotIn("foundation", excluded)
        self.assertNotIn("codec", excluded)
        self.assertEqual(excluded, {"proximadb", "embedded"})

    def test_ignores_non_workspace_dependency_names(self) -> None:
        # Third-party deps share the namespace in `dependencies`; only
        # workspace-internal edges may drive the closure.
        meta = _meta({"proximadb": [], "leaf": ["serde", "tokio"]})
        self.assertEqual(
            AFFECTED.workspace_dependents_of(meta, ["proximadb"]), {"proximadb"}
        )

    def test_unknown_root_is_a_noop(self) -> None:
        meta = _meta({"proximadb": [], "leaf": []})
        self.assertEqual(AFFECTED.workspace_dependents_of(meta, ["does-not-exist"]), set())

    def test_cycle_safe(self) -> None:
        # Cargo forbids cycles, but the traversal must terminate regardless.
        meta = _meta({"proximadb": ["a"], "a": ["b"], "b": ["proximadb"]})
        self.assertEqual(
            AFFECTED.workspace_dependents_of(meta, ["proximadb"]),
            {"proximadb", "a", "b"},
        )

    def test_multiple_roots(self) -> None:
        meta = _meta({"proximadb": [], "other": [], "x": ["other"], "leaf": []})
        self.assertEqual(
            AFFECTED.workspace_dependents_of(meta, ["proximadb", "other"]),
            {"proximadb", "other", "x"},
        )


class FastLaneWorkflowContractTest(unittest.TestCase):
    def test_production_scale_recall_stays_out_of_advisory_only(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        required_unit_job = workflow.split("\n  rust-test:", 1)[1].split(
            "\n  rust-affected-fast:", 1
        )[0]
        advisory_job = workflow.split("\n  rust-affected-fast:", 1)[1].split(
            "\n  object-store-emulator-tests:", 1
        )[0]
        slow_test = "rabitq_cold_recall_harness_n100000_recall_at_10"

        self.assertIn(
            "cargo nextest run --lib --profile unit --test-threads=2",
            required_unit_job,
            "the required unit lane must keep running every library test",
        )
        self.assertNotIn(slow_test, required_unit_job)
        self.assertIn(
            f"-E 'not test({slow_test})'",
            advisory_job,
            "the 10-minute advisory must not duplicate a production-scale ratchet",
        )


if __name__ == "__main__":
    unittest.main()
