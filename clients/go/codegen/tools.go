// Copyright 2025 Vijaykumar Singh
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//go:build tools

// Package codegen pins the OpenAPI -> Go code generator (TD-126 Phase 2).
//
// This is a standalone module (see go.mod) so the heavy generator dependency
// tree stays out of the SDK module's closure. The version of oapi-codegen used
// to generate ../proximadb/internal/genrest is pinned in this module's go.mod.
//
// Bumping the generator is a deliberate, reviewable change: update the require
// line in codegen/go.mod, run `make gen-go-sdk`, and commit the regenerated
// client together with the version bump. The CI drift gate regenerates and
// `git diff --exit-code`s the result, so the generated client cannot silently
// diverge from the published OpenAPI spec.
package codegen

import (
	_ "github.com/oapi-codegen/oapi-codegen/v2/cmd/oapi-codegen"
)
