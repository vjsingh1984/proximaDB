# Force bash so the verify-proto-sync recipe can use process substitution.
SHELL := /bin/bash

PROTO_DIR=../../proto
OUT_DIR=src
SDK_V1=$(OUT_DIR)/proximadb_sdk/v1
SDK_V2=$(OUT_DIR)/proximadb_sdk/v2
PUB_V1=$(OUT_DIR)/proximadb/v1
PUB_V2=$(OUT_DIR)/proximadb/v2

# Use the project venv so grpc_tools matches what tests/devs use.
PYTHON ?= /Users/vijaysingh/code/.venv/bin/python

# Full proto set for Python SDK generation
PROTO_SRC= \
	$(PROTO_DIR)/proximadb/v1/types.proto \
	$(PROTO_DIR)/proximadb/v1/collection_types.proto \
	$(PROTO_DIR)/proximadb/v1/vector_types.proto \
	$(PROTO_DIR)/proximadb/v1/vector.proto \
	$(PROTO_DIR)/proximadb/v1/sql.proto \
	$(PROTO_DIR)/proximadb/v1/collection.proto \
	$(PROTO_DIR)/proximadb/v1/entity.proto \
	$(PROTO_DIR)/proximadb/v1/relations.proto \
	$(PROTO_DIR)/proximadb/v1/graph.proto \
	$(PROTO_DIR)/proximadb/v1/graph_collection.proto \
	$(PROTO_DIR)/proximadb/v2/record.proto

# Files we maintain in the SDK-private copy. Anything not in this list
# (e.g. document_pb2 / hybrid_pb2) is hand-maintained outside the
# regenerator and left alone.
SDK_SYNCED=collection_pb2 collection_pb2_grpc collection_types_pb2 \
	collection_types_pb2_grpc entity_pb2 \
	entity_pb2_grpc graph_pb2 graph_pb2_grpc graph_collection_pb2 \
	graph_collection_pb2_grpc relations_pb2 relations_pb2_grpc sql_pb2 \
	sql_pb2_grpc types_pb2 types_pb2_grpc vector_pb2 vector_pb2_grpc \
	vector_types_pb2 vector_types_pb2_grpc
SDK_SYNCED_V2=record_pb2 record_pb2_grpc

.PHONY: gen-proto
gen-proto:
	@echo "Generating Python gRPC stubs into $(PUB_V1)/ and $(PUB_V2)/"
	$(PYTHON) -m grpc_tools.protoc -I $(PROTO_DIR) -I . \
		--python_out=$(OUT_DIR) \
		--grpc_python_out=$(OUT_DIR) \
		$(PROTO_SRC)
	@echo "Syncing SDK-private copy at $(SDK_V1)/ and $(SDK_V2)/"
	@for stem in $(SDK_SYNCED); do \
		cp $(PUB_V1)/$$stem.py $(SDK_V1)/$$stem.py; \
		sed -i.bak 's/from proximadb\.v1 import/from proximadb_sdk.v1 import/g; s/from proximadb\.v2 import/from proximadb_sdk.v2 import/g' $(SDK_V1)/$$stem.py; \
		rm -f $(SDK_V1)/$$stem.py.bak; \
	done
	@for stem in $(SDK_SYNCED_V2); do \
		cp $(PUB_V2)/$$stem.py $(SDK_V2)/$$stem.py; \
		sed -i.bak 's/from proximadb\.v1 import/from proximadb_sdk.v1 import/g; s/from proximadb\.v2 import/from proximadb_sdk.v2 import/g' $(SDK_V2)/$$stem.py; \
		rm -f $(SDK_V2)/$$stem.py.bak; \
	done
	@echo "Done — both proximadb/ and proximadb_sdk/ namespaces in sync."

.PHONY: verify-proto-sync
verify-proto-sync:
	@# Fail if any SDK-synced file diverges from the public copy modulo
	@# the namespace-rewrite. Run in CI to catch hand-edits that bypass
	@# `make gen-proto`.
	@fail=0; \
	for stem in $(SDK_SYNCED); do \
		diff <(sed 's/from proximadb_sdk\.v1 import/from proximadb.v1 import/g; s/from proximadb_sdk\.v2 import/from proximadb.v2 import/g' $(SDK_V1)/$$stem.py) $(PUB_V1)/$$stem.py >/dev/null 2>&1 \
			|| { echo "OUT OF SYNC: $$stem (run make gen-proto)"; fail=1; }; \
	done; \
	for stem in $(SDK_SYNCED_V2); do \
		diff <(sed 's/from proximadb_sdk\.v1 import/from proximadb.v1 import/g; s/from proximadb_sdk\.v2 import/from proximadb.v2 import/g' $(SDK_V2)/$$stem.py) $(PUB_V2)/$$stem.py >/dev/null 2>&1 \
			|| { echo "OUT OF SYNC: $$stem (run make gen-proto)"; fail=1; }; \
	done; \
	if [ $$fail -eq 0 ]; then echo "All synced files match."; else exit 1; fi
