#!/usr/bin/env python3
"""
Generate Python v2 protobuf files from .proto definitions.

This script automates the generation of Python protobuf bindings for v2 APIs.
It handles both protobuf messages (_pb2.py) and gRPC service stubs (_pb2_grpc.py).

Usage:
    python scripts/generate_v2_protos.py

The script will:
1. Generate protobuf files for v2 proto files
2. Fix import paths to work with the Python SDK structure
3. Place generated files in src/proximadb_sdk/v2/
"""

import subprocess
import sys
from pathlib import Path
from typing import List

# Root directories - use absolute paths from script location
SCRIPT_FILE = Path(__file__).resolve()
SCRIPT_DIR = SCRIPT_FILE.parent
PROJECT_ROOT = SCRIPT_DIR.parent.parent.parent  # Go up to repo root
PROTO_DIR = PROJECT_ROOT / "proto"
PYTHON_OUT = SCRIPT_DIR.parent / "src" / "proximadb_sdk" / "v2"

# v2 Proto files to generate (relative to proto directory)
PROTOS = [
    "proximadb/v2/record.proto",
    "proximadb/v2/graph.proto",
    "proximadb/v2/entity.proto",
    "proximadb/v2/document.proto",
]


def generate_proto(proto_path: str, proto_dir: Path, python_out: Path) -> bool:
    """Generate Python protobuf files for a single proto file.

    Args:
        proto_path: Path to the proto file (relative to proto_dir)
        proto_dir: Directory containing proto files
        python_out: Directory to write generated Python files

    Returns:
        True if generation succeeded, False otherwise
    """
    proto_file = proto_dir / proto_path

    if not proto_file.exists():
        print(f"Warning: Proto file not found: {proto_file}")
        return False

    print(f"Generating: {proto_path}")

    try:
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "grpc_tools.protoc",
                f"--proto_path={proto_dir}",
                f"--python_out={python_out}",
                f"--grpc_python_out={python_out}",
                str(proto_file),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        return True
    except subprocess.CalledProcessError as e:
        print(f"Error generating {proto_path}:")
        print(f"  stdout: {e.stdout}")
        print(f"  stderr: {e.stderr}")
        return False


def fix_imports(python_out: Path) -> None:
    """Fix import paths in generated proto files.

    The protoc compiler generates imports with 'from proximadb.v2 import ...'
    but we need 'from . import ...' for the Python SDK structure.

    Args:
        python_out: Directory containing generated Python files
    """
    print("Fixing import paths...")

    # Find all generated _pb2.py and _pb2_grpc.py files
    proto_files = list(python_out.glob("*_pb2.py")) + list(
        python_out.glob("*_pb2_grpc.py")
    )

    for proto_file in proto_files:
        content = proto_file.read_text()

        # Replace 'from proximadb.v2' with 'from .'
        content = content.replace("from proximadb.v2", "from .")

        # Replace 'import proximadb.v2' with 'from . import'
        content = content.replace("import proximadb.v2", "from . import")

        proto_file.write_text(content)

    print(f"Fixed imports in {len(proto_files)} files")


def main():
    """Main generation function."""
    print("🔄 Generating v2 Python protobuf files...")

    # Ensure output directory exists
    PYTHON_OUT.mkdir(parents=True, exist_ok=True)

    # Generate all proto files
    success_count = 0
    for proto in PROTOS:
        if generate_proto(proto, PROTO_DIR, PYTHON_OUT):
            success_count += 1

    print(f"\n✅ Generated {success_count}/{len(PROTOS)} proto files")

    # Fix imports
    fix_imports(PYTHON_OUT)

    print("\n📁 Generated files in src/proximadb_sdk/v2/:")
    for proto in PROTOS:
        proto_name = proto.replace("proximadb/v2/", "").replace(".proto", "")
        print(f"   - {proto_name}_pb2.py")
        print(f"   - {proto_name}_pb2_grpc.py")

    print("\n💡 Next steps:")
    print("   1. Verify the generated code compiles: python -m py_compile src/proximadb_sdk/v2/*")
    print("   2. Run tests to ensure nothing broke: pytest tests/")


if __name__ == "__main__":
    main()
