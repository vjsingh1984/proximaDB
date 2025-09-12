PROTO_DIR=../../proto
OUT_DIR=src

# Full proto set for Python SDK generation
PROTO_SRC= \
	$(PROTO_DIR)/proximadb.proto \
	$(PROTO_DIR)/proximadb/v1/types.proto \
	$(PROTO_DIR)/proximadb/v1/vector_types.proto \
	$(PROTO_DIR)/proximadb/v1/vector.proto \
	$(PROTO_DIR)/proximadb/v1/sql.proto \
	$(PROTO_DIR)/proximadb/v1/collection_types.proto \
	$(PROTO_DIR)/proximadb/v1/collection.proto \
	$(PROTO_DIR)/proximadb/v1/entity.proto \
	$(PROTO_DIR)/proximadb/v1/relations.proto \
	$(PROTO_DIR)/proximadb/v1/context.proto \
	$(PROTO_DIR)/proximadb/v1/graph.proto

.PHONY: gen-proto
gen-proto:
	@echo "Generating Python gRPC stubs from $(PROTO_SRC)"
	python -m grpc_tools.protoc -I $(PROTO_DIR) -I . \
		--python_out=$(OUT_DIR) \
		--grpc_python_out=$(OUT_DIR) \
		$(PROTO_SRC)
	@echo "Done."
