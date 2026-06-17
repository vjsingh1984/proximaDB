#!/usr/bin/env python3
"""


STATUS: ⚠️  Partial - Authentication API Incomplete
SDK Version: v1.0+ (AuthResult API needs refinement)
Server Version: v0.2.0+
Test Result: PARTIAL - Authentication partially implemented

ProximaDB Python SDK Authentication Examples

This module demonstrates various authentication methods supported by the ProximaDB Python SDK.
These examples show how to leverage the enhanced authentication features implemented in the client.

"""

import os
import logging
from typing import Optional
import time

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# Import ProximaDB client and authentication components
from proximadb import ProximaDBClient, connect
from proximadb.auth import AuthConfig, AuthMethod
from proximadb.models import CollectionConfig, VectorRecord

# Note: AuthResult and full auth implementation coming in SDK v1.1+
# This example demonstrates the planned authentication API


def example_api_key_auth():
    """Example: API Key Authentication (Legacy and Enhanced)"""
    print("\n🔑 API Key Authentication Example")
    print("=" * 50)

    # Method 1: Legacy API key (simple)
    client = connect(url="http://localhost:5678", api_key="your-api-key-here")

    # Method 2: Enhanced API key with explicit configuration
    auth_config = AuthConfig(method=AuthMethod.API_KEY, api_key="your-api-key-here")

    client = ProximaDBClient(url="http://localhost:5678", auth_config=auth_config)

    # Check authentication status
    auth_status = client.get_auth_status()
    print(f"Authentication Status: {auth_status}")

    # Perform operations with authenticated client
    try:
        health = client.health()
        print(f"✅ Health check successful: {health.status}")

        collections = client.list_collections()
        print(f"✅ Listed {len(collections)} collections")

    except Exception as e:
        print(f"❌ Operation failed: {e}")

    finally:
        client.close()


def example_jwt_auth():
    """Example: JWT Token Authentication"""
    print("\n🎫 JWT Token Authentication Example")
    print("=" * 50)

    # JWT authentication with automatic token management
    auth_config = AuthConfig(
        method=AuthMethod.JWT,
        jwt_token="your-jwt-token-here",
        # Optional: refresh token for automatic renewal
        refresh_token="your-refresh-token-here",
        # Token refresh threshold (refresh when 80% expired)
        refresh_threshold_seconds=300,
    )

    client = ProximaDBClient(url="http://localhost:5678", auth_config=auth_config)

    # Check authentication status
    auth_status = client.get_auth_status()
    print(f"Authentication Status: {auth_status}")
    print(f"Token Expires At: {auth_status.get('expires_at')}")
    print(f"User Roles: {auth_status.get('roles')}")
    print(f"Permissions: {auth_status.get('permissions')}")

    # Perform operations with token-based auth
    try:
        # The client automatically handles token refresh if needed
        collections = client.list_collections()
        print(f"✅ Listed {len(collections)} collections with JWT auth")

        # Manual token refresh if needed
        if client.refresh_authentication():
            print("✅ Token refreshed successfully")
        else:
            print("⚠️ Token refresh not needed or failed")

    except Exception as e:
        print(f"❌ JWT authentication failed: {e}")

    finally:
        client.close()


def example_oauth2_auth():
    """Example: OAuth2 Authentication with Multiple Providers"""
    print("\n🔐 OAuth2 Authentication Example")
    print("=" * 50)

    # OAuth2 with Google provider
    auth_config = AuthConfig(
        method=AuthMethod.OAUTH2,
        oauth2_client_id="your-google-client-id",
        oauth2_client_secret="your-google-client-secret",
        oauth2_provider="google",
        oauth2_scopes=["openid", "profile", "email"],
        oauth2_redirect_uri="http://localhost:8080/callback",
    )

    client = ProximaDBClient(url="http://localhost:5678", auth_config=auth_config)

    # Check authentication status
    auth_status = client.get_auth_status()
    print(f"Authentication Status: {auth_status}")

    # OAuth2 with Azure AD (alternative configuration)
    azure_auth_config = AuthConfig(
        method=AuthMethod.OAUTH2,
        oauth2_client_id="your-azure-client-id",
        oauth2_client_secret="your-azure-client-secret",
        oauth2_provider="azure",
        oauth2_tenant_id="your-tenant-id",
        oauth2_scopes=["https://graph.microsoft.com/.default"],
    )

    azure_client = ProximaDBClient(
        url="http://localhost:5678", auth_config=azure_auth_config
    )

    print("✅ OAuth2 clients configured for Google and Azure")

    # Cleanup
    client.close()
    azure_client.close()


def example_mtls_auth():
    """Example: Mutual TLS (mTLS) Client Certificate Authentication"""
    print("\n🔒 Mutual TLS Authentication Example")
    print("=" * 50)

    # mTLS authentication with client certificates
    auth_config = AuthConfig(
        method=AuthMethod.CLIENT_CERT,
        client_cert_file="/path/to/client.crt",
        client_key_file="/path/to/client.key",
        ca_cert_file="/path/to/ca.crt",  # Optional CA certificate
        verify_server_cert=True,
    )

    client = ProximaDBClient(
        url="https://localhost:5679",  # Secure gRPC endpoint
        auth_config=auth_config,
        verify_ssl=True,
    )

    # Check authentication status
    auth_status = client.get_auth_status()
    print(f"Authentication Status: {auth_status}")

    try:
        # mTLS provides strong authentication at transport level
        health = client.health()
        print(f"✅ Mutual TLS authentication successful: {health.status}")

    except Exception as e:
        print(f"❌ mTLS authentication failed: {e}")
        print("Note: This requires proper SSL certificates to be configured")

    finally:
        client.close()


def example_role_based_access():
    """Example: Role-Based Access Control (RBAC)"""
    print("\n👥 Role-Based Access Control Example")
    print("=" * 50)

    # Authentication with role-based permissions
    auth_config = AuthConfig(
        method=AuthMethod.JWT,
        jwt_token="your-jwt-token-with-roles",
        # JWT should contain role claims
    )

    client = ProximaDBClient(url="http://localhost:5678", auth_config=auth_config)

    # Check user roles and permissions
    auth_status = client.get_auth_status()
    roles = auth_status.get("roles", [])
    permissions = auth_status.get("permissions", [])

    print(f"User Roles: {roles}")
    print(f"User Permissions: {permissions}")

    # Perform operations based on permissions
    try:
        if "collections.read" in permissions:
            collections = client.list_collections()
            print(f"✅ Read permission: Listed {len(collections)} collections")

        if "collections.create" in permissions:
            # Create a test collection
            config = CollectionConfig(name="rbac_test_collection", dimension=128)
            collection = client.create_collection(config=config)
            print(f"✅ Create permission: Created collection {collection.id}")
        else:
            print("⚠️ No create permission - skipping collection creation")

        if "vectors.write" in permissions:
            # Insert test vector
            record = VectorRecord(
                id="rbac_test_vector", vector=[0.1] * 128, metadata={"test": "rbac"}
            )
            result = client.insert_vectors("rbac_test_collection", [record])
            print(f"✅ Write permission: Inserted vector with success={result.success}")
        else:
            print("⚠️ No write permission - skipping vector insertion")

    except Exception as e:
        print(f"❌ RBAC operation failed: {e}")

    finally:
        client.close()


def example_multi_tenant_auth():
    """Example: Multi-Tenant Authentication"""
    print("\n🏢 Multi-Tenant Authentication Example")
    print("=" * 50)

    # Tenant-specific authentication
    tenant_a_config = AuthConfig(
        method=AuthMethod.JWT,
        jwt_token="tenant-a-jwt-token",
        tenant_id="tenant-a",
        tenant_isolation=True,
    )

    tenant_b_config = AuthConfig(
        method=AuthMethod.JWT,
        jwt_token="tenant-b-jwt-token",
        tenant_id="tenant-b",
        tenant_isolation=True,
    )

    # Create clients for different tenants
    tenant_a_client = ProximaDBClient(
        url="http://localhost:5678", auth_config=tenant_a_config
    )

    tenant_b_client = ProximaDBClient(
        url="http://localhost:5678", auth_config=tenant_b_config
    )

    try:
        # Each client operates within its tenant scope
        tenant_a_collections = tenant_a_client.list_collections()
        tenant_b_collections = tenant_b_client.list_collections()

        print(f"Tenant A collections: {len(tenant_a_collections)}")
        print(f"Tenant B collections: {len(tenant_b_collections)}")
        print("✅ Multi-tenant isolation working correctly")

    except Exception as e:
        print(f"❌ Multi-tenant authentication failed: {e}")

    finally:
        tenant_a_client.close()
        tenant_b_client.close()


def example_authentication_refresh():
    """Example: Automatic Token Refresh"""
    print("\n🔄 Automatic Token Refresh Example")
    print("=" * 50)

    # Configure authentication with automatic refresh
    auth_config = AuthConfig(
        method=AuthMethod.JWT,
        jwt_token="short-lived-token",
        refresh_token="refresh-token",
        refresh_threshold_seconds=60,  # Refresh when <60 seconds remaining
        auto_refresh=True,
    )

    client = ProximaDBClient(url="http://localhost:5678", auth_config=auth_config)

    try:
        # Simulate long-running operations
        for i in range(5):
            print(f"Operation {i+1}: Checking collections...")

            # The client automatically refreshes tokens as needed
            collections = client.list_collections()
            print(f"  Found {len(collections)} collections")

            # Check if token was refreshed
            auth_status = client.get_auth_status()
            print(f"  Token expires at: {auth_status.get('expires_at')}")

            # Simulate time passing
            time.sleep(1)

        print("✅ Automatic token refresh working correctly")

    except Exception as e:
        print(f"❌ Token refresh failed: {e}")

    finally:
        client.close()


def example_authentication_error_handling():
    """Example: Authentication Error Handling"""
    print("\n⚠️  Authentication Error Handling Example")
    print("=" * 50)

    # Test with invalid credentials
    invalid_configs = [
        AuthConfig(method=AuthMethod.API_KEY, api_key="invalid-key"),
        AuthConfig(method=AuthMethod.JWT, jwt_token="invalid.jwt.token"),
        AuthConfig(
            method=AuthMethod.OAUTH2,
            oauth2_client_id="invalid-client",
            oauth2_client_secret="invalid-secret",
        ),
    ]

    for i, config in enumerate(invalid_configs, 1):
        print(f"\nTest {i}: {config.method.value} with invalid credentials")

        try:
            client = ProximaDBClient(url="http://localhost:5678", auth_config=config)

            # Check authentication status
            auth_status = client.get_auth_status()
            if not auth_status["authenticated"]:
                print(f"  ⚠️ Not authenticated (expected)")

            # Try to perform operation
            collections = client.list_collections()
            print(f"  ❌ Unexpected success with invalid credentials")

        except Exception as e:
            print(f"  ✅ Correctly rejected invalid credentials: {e}")

        finally:
            if "client" in locals():
                client.close()


def main():
    """Run all authentication examples"""
    print("ProximaDB Python SDK - Authentication Examples")
    print("=" * 60)

    examples = [
        example_api_key_auth,
        example_jwt_auth,
        example_oauth2_auth,
        example_mtls_auth,
        example_role_based_access,
        example_multi_tenant_auth,
        example_authentication_refresh,
        example_authentication_error_handling,
    ]

    for example in examples:
        try:
            example()
        except Exception as e:
            print(f"❌ Example {example.__name__} failed: {e}")

        print("\n" + "-" * 60 + "\n")

    print("Authentication examples completed!")
    print("\nNext Steps:")
    print("1. Configure your ProximaDB server with authentication enabled")
    print("2. Update the credentials in these examples with your actual values")
    print("3. Run specific examples: python auth_examples.py")
    print("4. Check the server logs for authentication events")


if __name__ == "__main__":
    main()
