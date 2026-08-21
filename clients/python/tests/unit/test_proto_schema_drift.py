"""Schema-level proto/stub drift gate (TD-PROTO-1).

`test_grpc_proto_drift.py` computes missing-stub / missing-servicer / missing-rpc.
All three are about **services and rpcs**, so a `.proto` change that adds a
message, a field or an enum and touches no service moves nothing in that drift
set and the gate is green by construction. `collection_types.proto`, where this
surfaced, declares no service at all — which is why a field sat unreflected in
the checked-in stub indefinitely.

This gate closes that hole by comparing the **serialized `FileDescriptorProto`**
embedded in each committed `*_pb2.py` against one generated from the `.proto` at
HEAD. Byte equality is the whole test:

* it needs no proto parsing, so it cannot drift from the schema language;
* it catches messages, fields, enums, rpcs **and options** together — the
  `deprecated: true` markers a hand-rolled comparison would miss;
* it is immune to protoc's *formatting*, which differs from the committed files
  and would otherwise dominate a textual diff. Of 26 files with a text diff when
  TD-PROTO-1 was measured, **12 had byte-identical descriptors**.

Skipped, not failed, when `grpcio-tools` is unavailable: the generator is a test
dependency and its absence is a missing tool, not drift.
"""

from __future__ import annotations

import ast
import re
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[4]
PROTO_DIR = REPO_ROOT / "proto"
PYTHON_SRC = REPO_ROOT / "clients" / "python" / "src"

#: Both packages ship generated stubs and both are live: `unified_client_async`,
#: `fusion_v2` and `document_v2` import from `proximadb.v2`, while the rest of
#: the SDK uses `proximadb_sdk.v*`. They are generated independently, which is
#: how they came to disagree — see `test_the_two_stub_trees_agree`.
STUB_PACKAGES = ("proximadb_sdk", "proximadb")

_SERIALIZED = re.compile(
    r"AddSerializedFile\(\s*(b'(?:[^'\\]|\\.)*'|b\"(?:[^\"\\]|\\.)*\")\s*\)", re.S
)

pytest.importorskip(
    "grpc_tools",
    reason="grpcio-tools generates the comparison stub; absence is not drift",
)


def _descriptor(path: Path) -> bytes | None:
    """The serialized FileDescriptorProto embedded in a generated stub."""
    match = _SERIALIZED.search(path.read_text())
    return ast.literal_eval(match.group(1)) if match else None


def _proto_files() -> list[Path]:
    return sorted(PROTO_DIR.rglob("*.proto"))


def _stub_for(proto: Path, package: str) -> Path:
    """Where the committed stub for ``proto`` lives, by convention."""
    rel = proto.relative_to(PROTO_DIR)  # proximadb/v1/foo.proto
    return PYTHON_SRC / package / Path(*rel.parts[1:]).with_name(rel.stem + "_pb2.py")


def _generate(proto: Path, out: Path) -> Path | None:
    """Generate a stub for ``proto`` into ``out``; None if protoc declines."""
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "grpc_tools.protoc",
            f"-I{PROTO_DIR}",
            f"--python_out={out}",
            str(proto),
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    rel = proto.relative_to(PROTO_DIR)
    generated = out / rel.with_name(rel.stem + "_pb2.py")
    return generated if generated.exists() else None


def _pairs() -> list[tuple[Path, str, Path]]:
    out = []
    for proto in _proto_files():
        for package in STUB_PACKAGES:
            stub = _stub_for(proto, package)
            if stub.exists():
                out.append((proto, package, stub))
    return out


PAIRS = _pairs()


def test_there_is_something_to_compare():
    """Guard the guard: a path convention change must not silently empty this."""
    assert PAIRS, (
        "No committed stub matched any .proto by the expected path convention. "
        "Either the layout moved or _stub_for is wrong — this gate is inert "
        "until that is fixed."
    )
    assert len({p for p, _, _ in PAIRS}) >= 10, (
        f"Only {len({p for p, _, _ in PAIRS})} protos matched a stub; the tree "
        "has far more. The convention is probably wrong for most of them."
    )


@pytest.mark.parametrize(
    "proto,package,stub",
    PAIRS,
    ids=[f"{pkg}/{p.relative_to(PROTO_DIR)}" for p, pkg, _ in PAIRS],
)
def test_committed_stub_matches_its_proto(proto: Path, package: str, stub: Path):
    """The committed stub must encode exactly the schema its .proto declares.

    This is the assertion the service/rpc gate cannot make. A message-only
    change — a new field, a new enum member, a `deprecated` option — leaves that
    gate green while the SDK silently loses the ability to see it.
    """
    with tempfile.TemporaryDirectory() as tmp:
        generated = _generate(proto, Path(tmp))
        if generated is None:
            pytest.skip(f"protoc could not generate {proto.name}")
        fresh = _descriptor(generated)
        committed = _descriptor(stub)

    assert committed is not None, f"{stub} has no serialized descriptor"
    assert fresh is not None, f"generated stub for {proto.name} has no descriptor"
    assert committed == fresh, (
        f"{stub.relative_to(REPO_ROOT)} no longer encodes "
        f"{proto.relative_to(REPO_ROOT)}.\n\n"
        "A .proto changed without its Python stub being regenerated. Fix with:\n"
        f"  python -m grpc_tools.protoc -Iproto --python_out=clients/python/src/{package} "
        f"{proto.relative_to(REPO_ROOT)}\n"
        "then move the file out of the nested proximadb/ directory protoc "
        f"creates, rewrite its `from proximadb.vN import` lines to `{package}`, "
        "and run BOTH isort and black over it -- the committed stubs carry that "
        "pass, and `isort --check-only src tests` is a CI gate, so protoc output "
        "alone is rejected.\n\n"
        "Descriptors are compared as bytes, so this is a real schema difference "
        "— formatting cannot trigger it."
    )


def test_the_two_stub_trees_agree():
    """`proximadb` and `proximadb_sdk` must encode the same contract.

    Both are live and both are generated independently, which is exactly how
    they drifted apart: when TD-PROTO-1 measured this, 24 of 26 descriptors
    matched and two did not. Collapsing the duplication is the real fix; this
    assertion is the cheap one that stops it recurring meanwhile.
    """
    mismatched = []
    for proto in _proto_files():
        a, b = (_stub_for(proto, pkg) for pkg in STUB_PACKAGES)
        if not (a.exists() and b.exists()):
            continue
        if _descriptor(a) != _descriptor(b):
            mismatched.append(str(proto.relative_to(PROTO_DIR)))
    assert not mismatched, (
        "The two generated stub trees encode different schemas for:\n  - "
        + "\n  - ".join(mismatched)
        + "\n\nThey are two copies of one contract. Regenerate both from the "
        "same .proto in the same change."
    )
