#!/usr/bin/env bash
# Copyright 2025 Vijaykumar Singh
#
# Licensed under the Apache License, Version 2.0 (the "License").
#
# Generate the typed TypeScript REST transport types (src/generated/schema.ts)
# from the published OpenAPI spec (TD-126 Phase 4). Invoked by `npm run gen-sdk`
# / `make gen-ts-sdk`.
#
# Unlike the Go pilot (TD-126 Phase 2), no 3.1 -> 3.0 down-conversion is needed:
# openapi-typescript is OpenAPI 3.1-native, so the source spec
# (docs/openapi/proximadb-openapi.yaml, generated + drift-gated by Phase 1) is
# consumed directly and never edited.
#
# Regenerating must be deterministic: the CI drift gate runs this and then
# `git diff --exit-code`s src/generated.
set -euo pipefail

# Resolve paths relative to this script so it works from any cwd.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${SDK_DIR}/../.." && pwd)"

SPEC="${REPO_ROOT}/docs/openapi/proximadb-openapi.yaml"
OUT="${SDK_DIR}/src/generated/schema.ts"

echo "==> generating ${OUT} (openapi-typescript, pinned in package.json)"
mkdir -p "$(dirname "${OUT}")"
( cd "${SDK_DIR}" && \
  npx --no-install openapi-typescript "${SPEC}" --output "${OUT}" )

echo "==> done"
