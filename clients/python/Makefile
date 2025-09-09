PROTO_SRC=\ 
	PROTO_SRC=\ 
	../../proto/proximadb.proto \ 
	../../proto/proximadb/v1/entity.proto \ 
	../../proto/proximadb/v1/relations.proto \ 
	../../proto/proximadb/v1/context.proto 
	../../proto/proximadb/v1/entity.proto \ 
	../../proto/proximadb/v1/relations.proto \ 
	../../proto/proximadb/v1/context.proto
OUT_DIR=src/proximadb

.PHONY: gen-proto
gen-proto:
	@echo "Generating Python gRPC stubs from $(PROTO_SRC)"
	python -m grpc_tools.protoc -I ../../proto -I . \
		--python_out=$(OUT_DIR) \
		--grpc_python_out=$(OUT_DIR) \
		$(PROTO_SRC)
	@echo "Done."

