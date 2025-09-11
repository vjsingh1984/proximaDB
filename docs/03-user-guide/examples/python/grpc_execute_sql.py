#!/usr/bin/env python3
# gRPC: ExecuteSql example (requires generated proximadb.v1 stubs)

import grpc
from proximadb_v1_pb2 import ExecuteSqlRequest, SqlValue
from proximadb_v1_pb2_grpc import SqlServiceStub

def main():
    channel = grpc.insecure_channel("localhost:5679")
    stub = SqlServiceStub(channel)

    # SELECT with parameterized vector
    vec = SqlValue(value=SqlValue.Value(number_value=0.1))
    req = ExecuteSqlRequest(
        query="SELECT id FROM my_collection ORDER BY COSINE_DISTANCE(vector, $1) LIMIT 5",
        parameters=[SqlValue(value=SqlValue.Value(array_value=SqlValue.Array(items=[vec])))],
        collection="my_collection",
    )
    resp = stub.ExecuteSql(req)
    print(resp)

if __name__ == "__main__":
    main()

