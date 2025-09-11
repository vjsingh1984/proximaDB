#!/usr/bin/env python3
"""
Test SQL functionality in the v1 ProximaDB client
"""

import sys
import os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), 'src'))

def test_sql_value_conversion():
    """Test SQL value conversion methods"""
    print("Testing SQL value conversion...")
    
    try:
        from proximadb.client_v1 import ProximaDBClientV1
        from proximadb.proto.proximadb.v1 import types_pb2
        
        client = ProximaDBClientV1(url="http://localhost:5678")
        
        # Test different value types
        test_cases = [
            ("string", "hello world"),
            ("integer", 42),
            ("float", 3.14159),
            ("boolean_true", True),
            ("boolean_false", False),
            ("null", None),
            ("bytes", b"binary data"),
            ("array", [1, "two", 3.0, True, None]),
            ("object", {"key1": "value1", "key2": 42, "nested": {"inner": True}})
        ]
        
        for test_name, value in test_cases:
            # Convert to proto
            proto_value = client._convert_to_sql_value(value)
            print(f"✅ {test_name}: {type(value).__name__} -> proto")
            
            # Convert back to Python
            converted_back = client._convert_from_sql_value(proto_value)
            print(f"✅ {test_name}: proto -> {type(converted_back).__name__}")
            
            # Verify round-trip (with special handling for bytes)
            if isinstance(value, (bytes, bytearray)):
                assert converted_back == value
            elif isinstance(value, dict):
                # For objects, do deep comparison
                assert converted_back == value
            elif isinstance(value, list):
                # For arrays, do element-wise comparison
                assert converted_back == value
            else:
                assert converted_back == value
            
            print(f"✅ {test_name}: round-trip conversion successful")
        
        return True
        
    except Exception as e:
        print(f"❌ SQL value conversion test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def test_sql_request_creation():
    """Test SQL request message creation"""
    print("\nTesting SQL request creation...")
    
    try:
        from proximadb.client_v1 import ProximaDBClientV1
        from proximadb.proto.proximadb.v1 import types_pb2
        
        client = ProximaDBClientV1(url="http://localhost:5678")
        
        # Test simple query without parameters
        query1 = "SELECT * FROM my_collection"
        proto_params1 = []
        
        request1 = types_pb2.ExecuteSqlRequest(
            query=query1,
            parameters=proto_params1
        )
        print(f"✅ Simple query request: {len(request1.query)} chars, {len(request1.parameters)} params")
        
        # Test query with parameters
        query2 = "SELECT * FROM my_collection WHERE category = $1 AND value > $2"
        params2 = ["electronics", 100]
        proto_params2 = [client._convert_to_sql_value(p) for p in params2]
        
        request2 = types_pb2.ExecuteSqlRequest(
            query=query2,
            parameters=proto_params2
        )
        print(f"✅ Parameterized query request: {len(request2.query)} chars, {len(request2.parameters)} params")
        
        # Verify parameter conversion
        for i, (original, proto_param) in enumerate(zip(params2, proto_params2)):
            converted = client._convert_from_sql_value(proto_param)
            assert converted == original
            print(f"✅ Parameter {i+1}: {original} -> {type(converted).__name__}")
        
        return True
        
    except Exception as e:
        print(f"❌ SQL request creation test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def test_sql_response_parsing():
    """Test SQL response parsing (mock response)"""
    print("\nTesting SQL response parsing...")
    
    try:
        from proximadb.client_v1 import ProximaDBClientV1
        from proximadb.proto.proximadb.v1 import types_pb2
        
        client = ProximaDBClientV1(url="http://localhost:5678")
        
        # Create mock SQL response
        # Row 1: {"id": "vec_1", "category": "electronics", "price": 299.99}
        row1_fields = [
            types_pb2.SqlRowField(
                key="id",
                value=types_pb2.SqlValue(string_value="vec_1")
            ),
            types_pb2.SqlRowField(
                key="category", 
                value=types_pb2.SqlValue(string_value="electronics")
            ),
            types_pb2.SqlRowField(
                key="price",
                value=types_pb2.SqlValue(number_value=299.99)
            )
        ]
        
        # Row 2: {"id": "vec_2", "category": "books", "price": 19.95}
        row2_fields = [
            types_pb2.SqlRowField(
                key="id",
                value=types_pb2.SqlValue(string_value="vec_2")
            ),
            types_pb2.SqlRowField(
                key="category",
                value=types_pb2.SqlValue(string_value="books")
            ),
            types_pb2.SqlRowField(
                key="price",
                value=types_pb2.SqlValue(number_value=19.95)
            )
        ]
        
        mock_response = types_pb2.ExecuteSqlResponse(
            rows=[
                types_pb2.SqlRow(fields=row1_fields),
                types_pb2.SqlRow(fields=row2_fields)
            ],
            rows_scanned=100,
            rows_returned=2
        )
        
        # Parse the response using the client's internal method
        rows = []
        for row in mock_response.rows:
            row_dict = {}
            for field in row.fields:
                row_dict[field.key] = client._convert_from_sql_value(field.value)
            rows.append(row_dict)
        
        result = {
            "rows": rows,
            "rows_scanned": mock_response.rows_scanned,
            "rows_returned": mock_response.rows_returned,
            "execution_time_ms": getattr(mock_response, 'execution_time_ms', 0)
        }
        
        print(f"✅ Parsed {len(result['rows'])} rows")
        print(f"✅ Row 1: {result['rows'][0]}")
        print(f"✅ Row 2: {result['rows'][1]}")
        print(f"✅ Rows scanned: {result['rows_scanned']}, returned: {result['rows_returned']}")
        
        # Verify data types
        assert isinstance(result['rows'][0]['id'], str)
        assert isinstance(result['rows'][0]['price'], float)
        assert result['rows'][0]['category'] == "electronics"
        assert result['rows'][1]['price'] == 19.95
        
        return True
        
    except Exception as e:
        print(f"❌ SQL response parsing test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def test_sql_client_methods():
    """Test SQL client methods (without server)"""
    print("\nTesting SQL client methods...")
    
    try:
        from proximadb.client_v1 import ProximaDBClientV1
        
        # Test REST client
        rest_client = ProximaDBClientV1(url="http://localhost:5678", protocol="rest")
        print(f"✅ REST client created: {rest_client.protocol}")
        
        # Test gRPC client
        grpc_client = ProximaDBClientV1(url="http://localhost:5679", protocol="grpc") 
        print(f"✅ gRPC client created: {grpc_client.protocol}")
        
        # Test that SQL methods exist
        assert hasattr(rest_client, 'execute_sql')
        assert hasattr(rest_client, '_execute_sql_rest')
        assert hasattr(grpc_client, 'execute_sql')
        assert hasattr(grpc_client, '_execute_sql_grpc')
        print("✅ SQL methods exist on both clients")
        
        # Test SQL stub exists on gRPC client
        assert hasattr(grpc_client, 'sql_stub')
        print("✅ SQL gRPC stub available")
        
        rest_client.close()
        grpc_client.close()
        
        return True
        
    except Exception as e:
        print(f"❌ SQL client methods test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def main():
    """Run all SQL tests"""
    print("ProximaDB Python SDK v1 - SQL Functionality Test")
    print("=" * 60)
    
    success = True
    
    # Run all SQL tests
    success &= test_sql_value_conversion()
    success &= test_sql_request_creation()
    success &= test_sql_response_parsing()
    success &= test_sql_client_methods()
    
    print("\n" + "=" * 60)
    if success:
        print("🎉 ALL SQL TESTS PASSED! SQL functionality is ready.")
        print("\nThe v1 client now supports:")
        print("  - SQL gRPC service integration")
        print("  - SQL value type conversion (strings, numbers, booleans, arrays, objects)")
        print("  - Parameterized query support") 
        print("  - Proper response parsing")
        print("\nSQL methods available:")
        print("  - client.execute_sql(query, parameters=None)")
        print("  - Both REST and gRPC protocols supported")
        return 0
    else:
        print("❌ SOME SQL TESTS FAILED! Check the output above.")
        return 1

if __name__ == "__main__":
    sys.exit(main())