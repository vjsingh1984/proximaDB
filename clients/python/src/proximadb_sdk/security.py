"""
ProximaDB Security Module

Comprehensive security features including:
- OAuth2 token refresh flow
- RBAC permission enforcement
- Audit logging
- mTLS verification
- Security context management

Copyright 2025 ProximaDB Contributors
Licensed under the Apache License, Version 2.0
"""

import functools
import hashlib
import hmac
import json
import logging
import os
import ssl
import threading
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Set, TypeVar, Union

logger = logging.getLogger(__name__)

F = TypeVar("F", bound=Callable[..., Any])


# =============================================================================
# OAuth2 Token Management
# =============================================================================


class OAuth2GrantType(Enum):
    """OAuth2 grant types."""

    AUTHORIZATION_CODE = "authorization_code"
    CLIENT_CREDENTIALS = "client_credentials"
    REFRESH_TOKEN = "refresh_token"
    PASSWORD = "password"  # Resource Owner Password (legacy)
    DEVICE_CODE = "device_code"


class OAuth2Provider(Enum):
    """Supported OAuth2 providers."""

    GENERIC = "generic"
    OKTA = "okta"
    AUTH0 = "auth0"
    AZURE_AD = "azure_ad"
    GOOGLE = "google"
    KEYCLOAK = "keycloak"
    COGNITO = "cognito"


@dataclass
class OAuth2TokenResponse:
    """OAuth2 token response."""

    access_token: str
    token_type: str = "Bearer"
    expires_in: Optional[int] = None
    refresh_token: Optional[str] = None
    scope: Optional[str] = None
    id_token: Optional[str] = None
    issued_at: datetime = field(default_factory=lambda: datetime.now(timezone.utc))

    @property
    def expires_at(self) -> Optional[datetime]:
        """Calculate expiration time."""
        if self.expires_in:
            return self.issued_at + timedelta(seconds=self.expires_in)
        return None

    @property
    def is_expired(self) -> bool:
        """Check if token is expired."""
        if self.expires_at is None:
            return False
        # Consider expired 30 seconds before actual expiry for safety
        return datetime.now(timezone.utc) >= (self.expires_at - timedelta(seconds=30))

    def time_until_expiry(self) -> Optional[timedelta]:
        """Get time until token expires."""
        if self.expires_at is None:
            return None
        return self.expires_at - datetime.now(timezone.utc)


@dataclass
class OAuth2Config:
    """OAuth2 configuration."""

    provider: OAuth2Provider = OAuth2Provider.GENERIC
    client_id: str = ""
    client_secret: Optional[str] = None
    token_url: Optional[str] = None
    authorize_url: Optional[str] = None
    userinfo_url: Optional[str] = None
    scopes: List[str] = field(default_factory=lambda: ["openid", "profile"])
    redirect_uri: Optional[str] = None
    audience: Optional[str] = None
    # Token refresh settings
    auto_refresh: bool = True
    refresh_threshold_seconds: int = 300  # Refresh 5 minutes before expiry
    max_refresh_attempts: int = 3
    # PKCE support
    use_pkce: bool = True

    def get_token_url(self) -> str:
        """Get token URL based on provider."""
        if self.token_url:
            return self.token_url
        # Well-known endpoints for common providers
        provider_urls = {
            OAuth2Provider.OKTA: f"https://{self.client_id.split('.')[0]}.okta.com/oauth2/default/v1/token",
            OAuth2Provider.AUTH0: f"https://{self.audience}/oauth/token",
            OAuth2Provider.GOOGLE: "https://oauth2.googleapis.com/token",
        }
        return provider_urls.get(self.provider, "")


class OAuth2TokenManager:
    """
    Manages OAuth2 token lifecycle including refresh.

    Features:
    - Automatic token refresh before expiry
    - Thread-safe token access
    - PKCE support for authorization code flow
    - Multiple provider support
    """

    def __init__(self, config: OAuth2Config):
        self.config = config
        self._token: Optional[OAuth2TokenResponse] = None
        self._lock = threading.RLock()
        self._refresh_callbacks: List[Callable[[OAuth2TokenResponse], None]] = []
        self._pkce_verifier: Optional[str] = None

    @property
    def token(self) -> Optional[OAuth2TokenResponse]:
        """Get current token, refreshing if needed."""
        with self._lock:
            if self._token and self._token.is_expired and self.config.auto_refresh:
                self._refresh_token()
            return self._token

    @token.setter
    def token(self, value: OAuth2TokenResponse):
        """Set token."""
        with self._lock:
            self._token = value

    def on_token_refresh(self, callback: Callable[[OAuth2TokenResponse], None]):
        """Register callback for token refresh events."""
        self._refresh_callbacks.append(callback)

    def exchange_code(
        self, code: str, code_verifier: Optional[str] = None
    ) -> OAuth2TokenResponse:
        """
        Exchange authorization code for tokens.

        Args:
            code: Authorization code from OAuth2 provider
            code_verifier: PKCE code verifier if using PKCE

        Returns:
            OAuth2TokenResponse with access and refresh tokens
        """
        import requests

        data = {
            "grant_type": "authorization_code",
            "code": code,
            "client_id": self.config.client_id,
            "redirect_uri": self.config.redirect_uri,
        }

        if self.config.client_secret:
            data["client_secret"] = self.config.client_secret

        if code_verifier or self._pkce_verifier:
            data["code_verifier"] = code_verifier or self._pkce_verifier

        response = requests.post(
            self.config.get_token_url(),
            data=data,
            headers={"Content-Type": "application/x-www-form-urlencoded"},
            timeout=30,
        )

        if response.status_code != 200:
            raise OAuth2Error(
                f"Token exchange failed: {response.status_code} - {response.text}"
            )

        token_data = response.json()
        self._token = OAuth2TokenResponse(
            access_token=token_data["access_token"],
            token_type=token_data.get("token_type", "Bearer"),
            expires_in=token_data.get("expires_in"),
            refresh_token=token_data.get("refresh_token"),
            scope=token_data.get("scope"),
            id_token=token_data.get("id_token"),
        )

        return self._token

    def client_credentials(self) -> OAuth2TokenResponse:
        """
        Get token using client credentials flow.

        Returns:
            OAuth2TokenResponse with access token
        """
        import requests

        if not self.config.client_secret:
            raise OAuth2Error("Client secret required for client credentials flow")

        data = {
            "grant_type": "client_credentials",
            "client_id": self.config.client_id,
            "client_secret": self.config.client_secret,
        }

        if self.config.scopes:
            data["scope"] = " ".join(self.config.scopes)

        if self.config.audience:
            data["audience"] = self.config.audience

        response = requests.post(
            self.config.get_token_url(),
            data=data,
            headers={"Content-Type": "application/x-www-form-urlencoded"},
            timeout=30,
        )

        if response.status_code != 200:
            raise OAuth2Error(
                f"Client credentials flow failed: {response.status_code} - {response.text}"
            )

        token_data = response.json()
        self._token = OAuth2TokenResponse(
            access_token=token_data["access_token"],
            token_type=token_data.get("token_type", "Bearer"),
            expires_in=token_data.get("expires_in"),
            scope=token_data.get("scope"),
        )

        return self._token

    def refresh(self) -> OAuth2TokenResponse:
        """
        Refresh the current token.

        Returns:
            New OAuth2TokenResponse

        Raises:
            OAuth2Error if refresh fails
        """
        with self._lock:
            return self._refresh_token()

    def _refresh_token(self) -> OAuth2TokenResponse:
        """Internal token refresh implementation."""
        import requests

        if not self._token or not self._token.refresh_token:
            raise OAuth2Error("No refresh token available")

        for attempt in range(self.config.max_refresh_attempts):
            try:
                data = {
                    "grant_type": "refresh_token",
                    "refresh_token": self._token.refresh_token,
                    "client_id": self.config.client_id,
                }

                if self.config.client_secret:
                    data["client_secret"] = self.config.client_secret

                response = requests.post(
                    self.config.get_token_url(),
                    data=data,
                    headers={"Content-Type": "application/x-www-form-urlencoded"},
                    timeout=30,
                )

                if response.status_code == 200:
                    token_data = response.json()
                    self._token = OAuth2TokenResponse(
                        access_token=token_data["access_token"],
                        token_type=token_data.get("token_type", "Bearer"),
                        expires_in=token_data.get("expires_in"),
                        refresh_token=token_data.get(
                            "refresh_token", self._token.refresh_token
                        ),
                        scope=token_data.get("scope"),
                        id_token=token_data.get("id_token"),
                    )

                    # Notify callbacks
                    for callback in self._refresh_callbacks:
                        try:
                            callback(self._token)
                        except Exception as e:
                            logger.warning(f"Token refresh callback failed: {e}")

                    return self._token

                if response.status_code == 400:
                    # Invalid refresh token - cannot retry
                    raise OAuth2Error(f"Invalid refresh token: {response.text}")

            except requests.exceptions.RequestException as e:
                if attempt == self.config.max_refresh_attempts - 1:
                    raise OAuth2Error(
                        f"Token refresh failed after {attempt + 1} attempts: {e}"
                    )
                time.sleep(2**attempt)  # Exponential backoff

        raise OAuth2Error("Token refresh failed: max attempts exceeded")

    def generate_pkce(self) -> tuple:
        """
        Generate PKCE code verifier and challenge.

        Returns:
            Tuple of (code_verifier, code_challenge)
        """
        import base64
        import secrets

        # Generate 32-byte random verifier
        verifier_bytes = secrets.token_bytes(32)
        code_verifier = (
            base64.urlsafe_b64encode(verifier_bytes).rstrip(b"=").decode("ascii")
        )

        # Create SHA256 challenge
        challenge_bytes = hashlib.sha256(code_verifier.encode("ascii")).digest()
        code_challenge = (
            base64.urlsafe_b64encode(challenge_bytes).rstrip(b"=").decode("ascii")
        )

        self._pkce_verifier = code_verifier
        return code_verifier, code_challenge


class OAuth2Error(Exception):
    """OAuth2-specific error."""

    pass


# =============================================================================
# RBAC Permission Enforcement
# =============================================================================


class Role(Enum):
    """Predefined roles."""

    ADMIN = "admin"
    DEVELOPER = "developer"
    ANALYST = "analyst"
    VIEWER = "viewer"
    SERVICE = "service"


@dataclass
class RoleDefinition:
    """Role with associated permissions."""

    name: str
    permissions: Set[str]
    description: str = ""
    inherits: Optional[List[str]] = None

    def __post_init__(self):
        if self.inherits is None:
            self.inherits = []


class RBACManager:
    """
    Role-Based Access Control manager.

    Features:
    - Role hierarchy with inheritance
    - Permission checking decorators
    - Dynamic role assignment
    - Audit trail for access decisions
    """

    # Default role definitions
    DEFAULT_ROLES = {
        Role.ADMIN.value: RoleDefinition(
            name="admin",
            permissions={
                "collection:*",
                "vector:*",
                "graph:*",
                "document:*",
                "observability:*",
                "system:*",
                "user:*",
                "role:*",
            },
            description="Full system access",
        ),
        Role.DEVELOPER.value: RoleDefinition(
            name="developer",
            permissions={
                "collection:create",
                "collection:read",
                "collection:update",
                "vector:*",
                "graph:*",
                "document:*",
                "observability:read",
            },
            description="Development access",
            inherits=["analyst"],
        ),
        Role.ANALYST.value: RoleDefinition(
            name="analyst",
            permissions={
                "collection:read",
                "vector:search",
                "vector:read",
                "graph:traverse",
                "graph:read",
                "document:read",
                "observability:read",
                "observability:query",
            },
            description="Read and query access",
            inherits=["viewer"],
        ),
        Role.VIEWER.value: RoleDefinition(
            name="viewer",
            permissions={"collection:list", "system:health"},
            description="View-only access",
        ),
        Role.SERVICE.value: RoleDefinition(
            name="service",
            permissions={
                "vector:insert",
                "vector:search",
                "graph:create",
                "graph:traverse",
                "document:create",
                "document:read",
                "observability:write",
            },
            description="Service account access",
        ),
    }

    def __init__(self, custom_roles: Optional[Dict[str, RoleDefinition]] = None):
        self._roles: Dict[str, RoleDefinition] = {**self.DEFAULT_ROLES}
        if custom_roles:
            self._roles.update(custom_roles)
        self._permission_cache: Dict[str, Set[str]] = {}
        self._audit_callback: Optional[Callable[[Dict[str, Any]], None]] = None

    def register_role(self, role: RoleDefinition):
        """Register a custom role."""
        self._roles[role.name] = role
        self._permission_cache.clear()  # Invalidate cache

    def get_effective_permissions(self, roles: List[str]) -> Set[str]:
        """
        Get all effective permissions for a set of roles.

        Includes inherited permissions from parent roles.
        """
        cache_key = ",".join(sorted(roles))
        if cache_key in self._permission_cache:
            return self._permission_cache[cache_key]

        permissions = set()
        visited = set()

        def collect_permissions(role_name: str):
            if role_name in visited:
                return
            visited.add(role_name)

            role_def = self._roles.get(role_name)
            if not role_def:
                return

            permissions.update(role_def.permissions)
            for parent in role_def.inherits or []:
                collect_permissions(parent)

        for role in roles:
            collect_permissions(role)

        self._permission_cache[cache_key] = permissions
        return permissions

    def check_permission(
        self,
        roles: List[str],
        required_permission: str,
        resource: Optional[str] = None,
    ) -> bool:
        """
        Check if roles have the required permission.

        Args:
            roles: List of role names
            required_permission: Required permission (e.g., "vector:search")
            resource: Optional specific resource to check

        Returns:
            True if permitted, False otherwise
        """
        permissions = self.get_effective_permissions(roles)

        # Check for wildcard permissions
        resource_type = required_permission.split(":")[0]
        if f"{resource_type}:*" in permissions:
            self._log_access_decision(roles, required_permission, resource, True)
            return True

        # Check exact permission
        result = required_permission in permissions
        self._log_access_decision(roles, required_permission, resource, result)
        return result

    def require_permission(self, permission: str):
        """
        Decorator to require a permission for a method.

        Usage:
            @rbac.require_permission("vector:search")
            def search_vectors(self, ...):
                ...
        """

        def decorator(func: F) -> F:
            @functools.wraps(func)
            def wrapper(*args, **kwargs):
                # Get security context from first argument (usually self)
                context = getattr(args[0], "_security_context", None) if args else None
                if context is None:
                    context = kwargs.get("security_context")

                if context is None:
                    raise PermissionError(
                        f"No security context for permission check: {permission}"
                    )

                if not self.check_permission(context.roles, permission):
                    raise PermissionError(f"Permission denied: {permission}")

                return func(*args, **kwargs)

            return wrapper

        return decorator

    def require_any_permission(self, permissions: List[str]):
        """Decorator requiring any of the listed permissions."""

        def decorator(func: F) -> F:
            @functools.wraps(func)
            def wrapper(*args, **kwargs):
                context = getattr(args[0], "_security_context", None) if args else None
                if context is None:
                    context = kwargs.get("security_context")

                if context is None:
                    raise PermissionError("No security context for permission check")

                for perm in permissions:
                    if self.check_permission(context.roles, perm):
                        return func(*args, **kwargs)

                raise PermissionError(
                    f"Permission denied: requires one of {permissions}"
                )

            return wrapper

        return decorator

    def set_audit_callback(self, callback: Callable[[Dict[str, Any]], None]):
        """Set callback for audit logging."""
        self._audit_callback = callback

    def _log_access_decision(
        self,
        roles: List[str],
        permission: str,
        resource: Optional[str],
        allowed: bool,
    ):
        """Log access decision for audit."""
        if self._audit_callback:
            self._audit_callback(
                {
                    "timestamp": datetime.now(timezone.utc).isoformat(),
                    "roles": roles,
                    "permission": permission,
                    "resource": resource,
                    "allowed": allowed,
                }
            )


# =============================================================================
# Security Context
# =============================================================================


@dataclass
class SecurityContext:
    """
    Security context for request processing.

    Contains authentication and authorization information
    for the current request/session.
    """

    user_id: str
    tenant_id: Optional[str] = None
    roles: List[str] = field(default_factory=list)
    permissions: Set[str] = field(default_factory=set)
    session_id: Optional[str] = None
    request_id: Optional[str] = None
    client_ip: Optional[str] = None
    user_agent: Optional[str] = None
    authenticated_at: datetime = field(
        default_factory=lambda: datetime.now(timezone.utc)
    )
    metadata: Dict[str, Any] = field(default_factory=dict)

    def has_permission(self, permission: str) -> bool:
        """Check if context has a specific permission."""
        resource_type = permission.split(":")[0]
        return (
            permission in self.permissions or f"{resource_type}:*" in self.permissions
        )

    def has_role(self, role: str) -> bool:
        """Check if context has a specific role."""
        return role in self.roles


# Thread-local storage for security context
_security_context_local = threading.local()


def get_current_security_context() -> Optional[SecurityContext]:
    """Get the current security context."""
    return getattr(_security_context_local, "context", None)


def set_security_context(context: SecurityContext):
    """Set the current security context."""
    _security_context_local.context = context


def clear_security_context():
    """Clear the current security context."""
    if hasattr(_security_context_local, "context"):
        del _security_context_local.context


class security_context:
    """
    Context manager for security context.

    Usage:
        with security_context(ctx):
            # Code runs with ctx as security context
            ...
    """

    def __init__(self, context: SecurityContext):
        self.context = context
        self._previous = None

    def __enter__(self):
        self._previous = get_current_security_context()
        set_security_context(self.context)
        return self.context

    def __exit__(self, exc_type, exc_val, exc_tb):
        if self._previous:
            set_security_context(self._previous)
        else:
            clear_security_context()
        return False


# =============================================================================
# Audit Logging
# =============================================================================


class AuditEventType(Enum):
    """Types of audit events."""

    AUTHENTICATION = "authentication"
    AUTHORIZATION = "authorization"
    DATA_ACCESS = "data_access"
    DATA_MODIFICATION = "data_modification"
    CONFIGURATION = "configuration"
    SECURITY = "security"
    SYSTEM = "system"


@dataclass
class AuditEvent:
    """Audit event record."""

    event_id: str
    event_type: AuditEventType
    timestamp: datetime
    user_id: str
    tenant_id: Optional[str]
    action: str
    resource_type: str
    resource_id: Optional[str]
    outcome: str  # "success", "failure", "denied"
    client_ip: Optional[str] = None
    user_agent: Optional[str] = None
    request_id: Optional[str] = None
    session_id: Optional[str] = None
    details: Dict[str, Any] = field(default_factory=dict)
    metadata: Dict[str, Any] = field(default_factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary."""
        return {
            "event_id": self.event_id,
            "event_type": self.event_type.value,
            "timestamp": self.timestamp.isoformat(),
            "user_id": self.user_id,
            "tenant_id": self.tenant_id,
            "action": self.action,
            "resource_type": self.resource_type,
            "resource_id": self.resource_id,
            "outcome": self.outcome,
            "client_ip": self.client_ip,
            "user_agent": self.user_agent,
            "request_id": self.request_id,
            "session_id": self.session_id,
            "details": self.details,
            "metadata": self.metadata,
        }

    def to_json(self) -> str:
        """Convert to JSON string."""
        return json.dumps(self.to_dict())


class AuditLogger:
    """
    Audit logger for security events.

    Features:
    - Multiple output destinations (file, remote, callback)
    - Structured logging format
    - Async batch processing
    - Tamper-evident logging (optional)
    """

    def __init__(
        self,
        log_file: Optional[str] = None,
        remote_endpoint: Optional[str] = None,
        enable_signing: bool = False,
        signing_key: Optional[bytes] = None,
    ):
        self._log_file = log_file
        self._remote_endpoint = remote_endpoint
        self._enable_signing = enable_signing
        self._signing_key = signing_key
        self._callbacks: List[Callable[[AuditEvent], None]] = []
        self._lock = threading.Lock()
        self._batch: List[AuditEvent] = []
        self._batch_size = 100
        self._last_hash: Optional[str] = None

    def log(
        self,
        event_type: AuditEventType,
        action: str,
        resource_type: str,
        resource_id: Optional[str] = None,
        outcome: str = "success",
        details: Optional[Dict[str, Any]] = None,
    ) -> AuditEvent:
        """
        Log an audit event.

        Args:
            event_type: Type of event
            action: Action performed
            resource_type: Type of resource
            resource_id: Optional resource identifier
            outcome: Outcome of the action
            details: Additional details

        Returns:
            Created AuditEvent
        """
        context = get_current_security_context()

        event = AuditEvent(
            event_id=str(uuid.uuid4()),
            event_type=event_type,
            timestamp=datetime.now(timezone.utc),
            user_id=context.user_id if context else "system",
            tenant_id=context.tenant_id if context else None,
            action=action,
            resource_type=resource_type,
            resource_id=resource_id,
            outcome=outcome,
            client_ip=context.client_ip if context else None,
            user_agent=context.user_agent if context else None,
            request_id=context.request_id if context else None,
            session_id=context.session_id if context else None,
            details=details or {},
        )

        # Add chain hash for tamper-evidence
        if self._enable_signing:
            event.metadata["chain_hash"] = self._compute_chain_hash(event)

        self._process_event(event)
        return event

    def log_authentication(
        self,
        user_id: str,
        auth_method: str,
        outcome: str,
        details: Optional[Dict[str, Any]] = None,
    ) -> AuditEvent:
        """Log authentication event."""
        return self.log(
            event_type=AuditEventType.AUTHENTICATION,
            action=f"authenticate_{auth_method}",
            resource_type="auth",
            resource_id=user_id,
            outcome=outcome,
            details=details,
        )

    def log_authorization(
        self,
        permission: str,
        resource_id: Optional[str],
        allowed: bool,
    ) -> AuditEvent:
        """Log authorization event."""
        return self.log(
            event_type=AuditEventType.AUTHORIZATION,
            action=f"check_{permission}",
            resource_type=permission.split(":")[0],
            resource_id=resource_id,
            outcome="success" if allowed else "denied",
        )

    def log_data_access(
        self,
        action: str,
        resource_type: str,
        resource_id: Optional[str],
        details: Optional[Dict[str, Any]] = None,
    ) -> AuditEvent:
        """Log data access event."""
        return self.log(
            event_type=AuditEventType.DATA_ACCESS,
            action=action,
            resource_type=resource_type,
            resource_id=resource_id,
            details=details,
        )

    def on_event(self, callback: Callable[[AuditEvent], None]):
        """Register callback for audit events."""
        self._callbacks.append(callback)

    def _process_event(self, event: AuditEvent):
        """Process an audit event."""
        with self._lock:
            # Write to file
            if self._log_file:
                self._write_to_file(event)

            # Send to remote
            if self._remote_endpoint:
                self._batch.append(event)
                if len(self._batch) >= self._batch_size:
                    self._flush_batch()

            # Call callbacks
            for callback in self._callbacks:
                try:
                    callback(event)
                except Exception as e:
                    logger.warning(f"Audit callback failed: {e}")

    def _write_to_file(self, event: AuditEvent):
        """Write event to log file."""
        try:
            with open(self._log_file, "a") as f:
                f.write(event.to_json() + "\n")
        except Exception as e:
            logger.error(f"Failed to write audit log: {e}")

    def _flush_batch(self):
        """Flush batch to remote endpoint."""
        if not self._batch:
            return

        try:
            import requests

            events = [e.to_dict() for e in self._batch]
            response = requests.post(
                self._remote_endpoint,
                json={"events": events},
                timeout=10,
            )
            if response.status_code == 200:
                self._batch.clear()
        except Exception as e:
            logger.error(f"Failed to flush audit batch: {e}")

    def _compute_chain_hash(self, event: AuditEvent) -> str:
        """Compute chain hash for tamper-evidence."""
        data = f"{self._last_hash or ''}{event.event_id}{event.timestamp.isoformat()}"
        if self._signing_key:
            h = hmac.new(self._signing_key, data.encode(), hashlib.sha256)
        else:
            h = hashlib.sha256(data.encode())
        self._last_hash = h.hexdigest()
        return self._last_hash


# =============================================================================
# mTLS Configuration
# =============================================================================


@dataclass
class MTLSConfig:
    """mTLS configuration."""

    enabled: bool = False
    client_cert_path: Optional[str] = None
    client_key_path: Optional[str] = None
    ca_cert_path: Optional[str] = None
    verify_hostname: bool = True
    check_revocation: bool = True
    min_tls_version: str = "TLSv1.2"
    allowed_ciphers: Optional[List[str]] = None

    def create_ssl_context(self) -> ssl.SSLContext:
        """Create configured SSL context."""
        # Determine minimum TLS version
        min_version = {
            "TLSv1.2": ssl.TLSVersion.TLSv1_2,
            "TLSv1.3": ssl.TLSVersion.TLSv1_3,
        }.get(self.min_tls_version, ssl.TLSVersion.TLSv1_2)

        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.minimum_version = min_version

        # Load client certificate
        if self.client_cert_path and self.client_key_path:
            context.load_cert_chain(
                certfile=self.client_cert_path,
                keyfile=self.client_key_path,
            )

        # Load CA certificates
        if self.ca_cert_path:
            context.load_verify_locations(cafile=self.ca_cert_path)
        else:
            context.load_default_certs()

        # Configure verification
        context.check_hostname = self.verify_hostname
        context.verify_mode = ssl.CERT_REQUIRED

        # Set allowed ciphers
        if self.allowed_ciphers:
            context.set_ciphers(":".join(self.allowed_ciphers))

        return context

    def validate(self) -> List[str]:
        """Validate configuration, returning list of issues."""
        issues = []

        if self.enabled:
            if self.client_cert_path and not Path(self.client_cert_path).exists():
                issues.append(f"Client certificate not found: {self.client_cert_path}")
            if self.client_key_path and not Path(self.client_key_path).exists():
                issues.append(f"Client key not found: {self.client_key_path}")
            if self.ca_cert_path and not Path(self.ca_cert_path).exists():
                issues.append(f"CA certificate not found: {self.ca_cert_path}")

        return issues


# =============================================================================
# Unified Security Manager
# =============================================================================


class SecurityManager:
    """
    Unified security manager combining all security features.

    Usage:
        security = SecurityManager(
            oauth2_config=OAuth2Config(...),
            rbac_manager=RBACManager(),
            audit_logger=AuditLogger(...),
            mtls_config=MTLSConfig(...),
        )

        # Authenticate
        token = security.oauth2.client_credentials()

        # Create context
        ctx = security.create_context(user_id="user1", roles=["developer"])

        # Check permissions
        with security_context(ctx):
            if security.check_permission("vector:search"):
                # Perform search
                ...
    """

    def __init__(
        self,
        oauth2_config: Optional[OAuth2Config] = None,
        rbac_manager: Optional[RBACManager] = None,
        audit_logger: Optional[AuditLogger] = None,
        mtls_config: Optional[MTLSConfig] = None,
    ):
        self.oauth2 = OAuth2TokenManager(oauth2_config) if oauth2_config else None
        self.rbac = rbac_manager or RBACManager()
        self.audit = audit_logger or AuditLogger()
        self.mtls = mtls_config

        # Connect RBAC to audit logger
        self.rbac.set_audit_callback(
            lambda event: self.audit.log_authorization(
                permission=event["permission"],
                resource_id=event.get("resource"),
                allowed=event["allowed"],
            )
        )

    def create_context(
        self,
        user_id: str,
        tenant_id: Optional[str] = None,
        roles: Optional[List[str]] = None,
        client_ip: Optional[str] = None,
        user_agent: Optional[str] = None,
    ) -> SecurityContext:
        """Create a security context."""
        roles = roles or []
        permissions = self.rbac.get_effective_permissions(roles)

        return SecurityContext(
            user_id=user_id,
            tenant_id=tenant_id,
            roles=roles,
            permissions=permissions,
            request_id=str(uuid.uuid4()),
            client_ip=client_ip,
            user_agent=user_agent,
        )

    def check_permission(self, permission: str, resource: Optional[str] = None) -> bool:
        """Check permission using current context."""
        context = get_current_security_context()
        if context is None:
            return False
        return self.rbac.check_permission(context.roles, permission, resource)

    def require_permission(self, permission: str):
        """Decorator requiring a permission."""
        return self.rbac.require_permission(permission)

    def get_ssl_context(self) -> Optional[ssl.SSLContext]:
        """Get SSL context if mTLS is configured."""
        if self.mtls and self.mtls.enabled:
            return self.mtls.create_ssl_context()
        return None
