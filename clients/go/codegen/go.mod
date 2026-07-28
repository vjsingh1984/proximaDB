// Isolated module for the OpenAPI -> Go code generator (TD-126 Phase 2).
//
// Keeping the generator in its own module means the SDK module
// (github.com/proximadb/proximadb-go) only depends on the small generated-code
// runtime (github.com/oapi-codegen/runtime), not the entire oapi-codegen
// toolchain (kin-openapi, speakeasy, etc.). `make gen-go-sdk` runs the generator
// from here; the version below is the single pin.
module github.com/proximadb/proximadb-go/codegen

go 1.25

require github.com/oapi-codegen/oapi-codegen/v2 v2.4.1

require (
	github.com/dprotaso/go-yit v0.0.0-20220510233725-9ba8df137936 // indirect
	github.com/getkin/kin-openapi v0.144.0 // indirect
	github.com/go-openapi/jsonpointer v0.22.5 // indirect
	github.com/go-openapi/swag/jsonname v0.25.5 // indirect
	github.com/oasdiff/yaml v0.1.1 // indirect
	github.com/oasdiff/yaml3 v0.0.14 // indirect
	github.com/santhosh-tekuri/jsonschema/v6 v6.0.2 // indirect
	github.com/speakeasy-api/openapi-overlay v0.9.0 // indirect
	github.com/vmware-labs/yaml-jsonpath v0.3.2 // indirect
	golang.org/x/mod v0.17.0 // indirect
	golang.org/x/text v0.18.0 // indirect
	golang.org/x/tools v0.21.1-0.20240508182429-e35e4ccd0d2d // indirect
	gopkg.in/yaml.v2 v2.4.0 // indirect
	gopkg.in/yaml.v3 v3.0.1 // indirect
)
