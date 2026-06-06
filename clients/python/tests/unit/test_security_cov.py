"""Offline unit tests for proximadb_sdk.security.

Fully offline: every network call (requests.post) is monkeypatched.
"""

from datetime import datetime, timedelta, timezone

import pytest

from proximadb_sdk import security as sec
from proximadb_sdk.security import (
    AuditEvent,
    AuditEventType,
    AuditLogger,
    MTLSConfig,
    OAuth2Config,
    OAuth2Error,
    OAuth2GrantType,
    OAuth2Provider,
    OAuth2TokenManager,
    OAuth2TokenResponse,
    RBACManager,
    Role,
    RoleDefinition,
    SecurityContext,
    SecurityManager,
    clear_security_context,
    get_current_security_context,
    security_context,
    set_security_context,
)


# ---------------------------------------------------------------------------
# Fake requests.post infrastructure
# ---------------------------------------------------------------------------


class FakeResp:
    def __init__(self, status_code=200, payload=None, text=""):
        self.status_code = status_code
        self._payload = payload or {}
        self.text = text

    def json(self):
        return self._payload


def make_post(responses):
    """Return a fake requests.post that pops from a list of FakeResp."""
    calls = []

    def _post(url, data=None, headers=None, json=None, timeout=None):
        calls.append({"url": url, "data": data, "json": json})
        resp = responses.pop(0)
        if isinstance(resp, Exception):
            raise resp
        return resp

    _post.calls = calls
    return _post


@pytest.fixture
def fake_requests(monkeypatch):
    """Provides a module object with a patchable .post; patched into security via import requests."""
    import requests

    holder = {}

    def install(responses):
        post = make_post(list(responses))
        monkeypatch.setattr(requests, "post", post)
        holder["post"] = post
        return post

    holder["install"] = install
    return holder


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------


def test_enums():
    assert OAuth2GrantType.AUTHORIZATION_CODE.value == "authorization_code"
    assert OAuth2GrantType.REFRESH_TOKEN.value == "refresh_token"
    assert OAuth2Provider.OKTA.value == "okta"
    assert Role.ADMIN.value == "admin"
    assert AuditEventType.AUTHENTICATION.value == "authentication"


# ---------------------------------------------------------------------------
# OAuth2TokenResponse
# ---------------------------------------------------------------------------


def test_token_response_no_expiry():
    t = OAuth2TokenResponse(access_token="a")
    assert t.expires_at is None
    assert t.is_expired is False
    assert t.time_until_expiry() is None


def test_token_response_future_expiry():
    t = OAuth2TokenResponse(access_token="a", expires_in=3600)
    assert t.expires_at is not None
    assert t.is_expired is False
    delta = t.time_until_expiry()
    assert delta is not None and delta.total_seconds() > 0


def test_token_response_expired():
    t = OAuth2TokenResponse(
        access_token="a",
        expires_in=10,
        issued_at=datetime.now(timezone.utc) - timedelta(seconds=100),
    )
    assert t.is_expired is True


# ---------------------------------------------------------------------------
# OAuth2Config
# ---------------------------------------------------------------------------


def test_config_explicit_token_url():
    cfg = OAuth2Config(token_url="https://x/token")
    assert cfg.get_token_url() == "https://x/token"


def test_config_provider_urls():
    okta = OAuth2Config(provider=OAuth2Provider.OKTA, client_id="myorg.app")
    assert "myorg.okta.com" in okta.get_token_url()

    auth0 = OAuth2Config(provider=OAuth2Provider.AUTH0, audience="tenant.auth0.com")
    assert "tenant.auth0.com/oauth/token" in auth0.get_token_url()

    google = OAuth2Config(provider=OAuth2Provider.GOOGLE)
    assert google.get_token_url() == "https://oauth2.googleapis.com/token"


def test_config_generic_no_url():
    cfg = OAuth2Config(provider=OAuth2Provider.GENERIC)
    assert cfg.get_token_url() == ""


# ---------------------------------------------------------------------------
# OAuth2TokenManager
# ---------------------------------------------------------------------------


def _mgr(**kw):
    cfg = OAuth2Config(
        client_id="cid",
        client_secret="secret",
        token_url="https://idp/token",
        **kw,
    )
    return OAuth2TokenManager(cfg)


def test_generate_pkce():
    m = _mgr()
    verifier, challenge = m.generate_pkce()
    assert isinstance(verifier, str) and isinstance(challenge, str)
    assert m._pkce_verifier == verifier


def test_exchange_code_success(fake_requests):
    fake_requests["install"](
        [FakeResp(200, {"access_token": "AT", "refresh_token": "RT", "expires_in": 60})]
    )
    m = _mgr(redirect_uri="https://cb")
    tok = m.exchange_code("the-code", code_verifier="ver")
    assert tok.access_token == "AT"
    assert tok.refresh_token == "RT"
    assert m._token is tok


def test_exchange_code_uses_stored_pkce(fake_requests):
    fake_requests["install"]([FakeResp(200, {"access_token": "AT"})])
    m = _mgr(redirect_uri="https://cb")
    m.generate_pkce()
    m.exchange_code("c")
    assert fake_requests["post"].calls[0]["data"]["code_verifier"]


def test_exchange_code_failure(fake_requests):
    fake_requests["install"]([FakeResp(401, {}, text="nope")])
    m = _mgr(redirect_uri="https://cb")
    with pytest.raises(OAuth2Error):
        m.exchange_code("c")


def test_client_credentials_success(fake_requests):
    fake_requests["install"](
        [FakeResp(200, {"access_token": "CT", "expires_in": 120, "scope": "read"})]
    )
    m = _mgr(audience="aud")
    tok = m.client_credentials()
    assert tok.access_token == "CT"
    # scope + audience were included in data
    data = fake_requests["post"].calls[0]["data"]
    assert "scope" in data and data["audience"] == "aud"


def test_client_credentials_requires_secret():
    cfg = OAuth2Config(client_id="cid", token_url="https://idp/token")
    m = OAuth2TokenManager(cfg)
    with pytest.raises(OAuth2Error):
        m.client_credentials()


def test_client_credentials_failure(fake_requests):
    fake_requests["install"]([FakeResp(500, {}, text="boom")])
    m = _mgr()
    with pytest.raises(OAuth2Error):
        m.client_credentials()


def test_refresh_success_with_callback(fake_requests):
    fake_requests["install"](
        [FakeResp(200, {"access_token": "NEW", "expires_in": 60})]
    )
    m = _mgr()
    m._token = OAuth2TokenResponse(access_token="OLD", refresh_token="RT")
    received = []
    m.on_token_refresh(lambda t: received.append(t))
    tok = m.refresh()
    assert tok.access_token == "NEW"
    # refresh_token preserved from old token (not in response)
    assert tok.refresh_token == "RT"
    assert received and received[0] is tok


def test_refresh_callback_exception_swallowed(fake_requests):
    fake_requests["install"]([FakeResp(200, {"access_token": "NEW"})])
    m = _mgr()
    m._token = OAuth2TokenResponse(access_token="OLD", refresh_token="RT")

    def bad(_t):
        raise ValueError("cb fail")

    m.on_token_refresh(bad)
    tok = m.refresh()  # should not raise
    assert tok.access_token == "NEW"


def test_refresh_no_token():
    m = _mgr()
    with pytest.raises(OAuth2Error):
        m.refresh()


def test_refresh_invalid_400(fake_requests):
    fake_requests["install"]([FakeResp(400, {}, text="bad refresh")])
    m = _mgr()
    m._token = OAuth2TokenResponse(access_token="OLD", refresh_token="RT")
    with pytest.raises(OAuth2Error):
        m.refresh()


def test_refresh_network_retry_then_fail(fake_requests, monkeypatch):
    import requests

    # Avoid real sleeping
    monkeypatch.setattr(sec.time, "sleep", lambda *_a, **_k: None)
    exc = requests.exceptions.RequestException("conn")
    fake_requests["install"]([exc, exc, exc])
    m = _mgr(max_refresh_attempts=3)
    m._token = OAuth2TokenResponse(access_token="OLD", refresh_token="RT")
    with pytest.raises(OAuth2Error):
        m.refresh()


def test_refresh_network_retry_then_success(fake_requests, monkeypatch):
    import requests

    monkeypatch.setattr(sec.time, "sleep", lambda *_a, **_k: None)
    fake_requests["install"](
        [
            requests.exceptions.RequestException("conn"),
            FakeResp(200, {"access_token": "RECOVERED"}),
        ]
    )
    m = _mgr(max_refresh_attempts=3)
    m._token = OAuth2TokenResponse(access_token="OLD", refresh_token="RT")
    tok = m.refresh()
    assert tok.access_token == "RECOVERED"


def test_token_property_autorefresh(fake_requests):
    fake_requests["install"]([FakeResp(200, {"access_token": "FRESH", "expires_in": 60})])
    m = _mgr(auto_refresh=True)
    m._token = OAuth2TokenResponse(
        access_token="OLD",
        refresh_token="RT",
        expires_in=10,
        issued_at=datetime.now(timezone.utc) - timedelta(seconds=100),
    )
    tok = m.token
    assert tok.access_token == "FRESH"


def test_token_property_setter_and_no_refresh():
    m = _mgr()
    tr = OAuth2TokenResponse(access_token="X")
    m.token = tr
    assert m.token is tr


# ---------------------------------------------------------------------------
# RBAC
# ---------------------------------------------------------------------------


def test_rbac_effective_permissions_inheritance():
    r = RBACManager()
    perms = r.get_effective_permissions(["developer"])
    # developer inherits analyst -> viewer
    assert "collection:create" in perms
    assert "vector:search" in perms  # from analyst
    assert "system:health" in perms  # from viewer
    # cache hit on 2nd call
    assert r.get_effective_permissions(["developer"]) is perms


def test_rbac_unknown_role():
    r = RBACManager()
    assert r.get_effective_permissions(["nope"]) == set()


def test_rbac_check_permission_wildcard():
    r = RBACManager()
    assert r.check_permission(["admin"], "vector:search") is True


def test_rbac_check_permission_exact_and_deny():
    r = RBACManager()
    assert r.check_permission(["viewer"], "collection:list") is True
    assert r.check_permission(["viewer"], "vector:delete") is False


def test_rbac_register_custom_role_clears_cache():
    r = RBACManager()
    r.get_effective_permissions(["viewer"])  # populate cache
    r.register_role(RoleDefinition(name="custom", permissions={"foo:bar"}))
    assert r.check_permission(["custom"], "foo:bar") is True


def test_rbac_custom_roles_ctor():
    r = RBACManager(custom_roles={"x": RoleDefinition(name="x", permissions={"a:b"})})
    assert r.check_permission(["x"], "a:b") is True


def test_rbac_audit_callback():
    r = RBACManager()
    events = []
    r.set_audit_callback(lambda e: events.append(e))
    r.check_permission(["viewer"], "collection:list", resource="col1")
    assert events and events[-1]["allowed"] is True
    assert events[-1]["resource"] == "col1"


def test_role_definition_post_init():
    rd = RoleDefinition(name="z", permissions=set())
    assert rd.inherits == []
    rd2 = RoleDefinition(name="z", permissions=set(), inherits=["a"])
    assert rd2.inherits == ["a"]


def test_rbac_require_permission_decorator():
    r = RBACManager()

    class Svc:
        def __init__(self, ctx):
            self._security_context = ctx

        @r.require_permission("vector:search")
        def do(self):
            return "ok"

    ctx_ok = SecurityContext(user_id="u", roles=["analyst"])
    assert Svc(ctx_ok).do() == "ok"

    ctx_bad = SecurityContext(user_id="u", roles=["viewer"])
    with pytest.raises(PermissionError):
        Svc(ctx_bad).do()


def test_rbac_require_permission_no_context():
    r = RBACManager()

    @r.require_permission("vector:search")
    def f(security_context=None):
        return "ok"

    with pytest.raises(PermissionError):
        f()
    assert f(security_context=SecurityContext(user_id="u", roles=["analyst"])) == "ok"


def test_rbac_require_any_permission():
    r = RBACManager()

    class Svc:
        def __init__(self, ctx):
            self._security_context = ctx

        @r.require_any_permission(["vector:delete", "vector:search"])
        def do(self):
            return "ok"

    assert Svc(SecurityContext(user_id="u", roles=["analyst"])).do() == "ok"
    with pytest.raises(PermissionError):
        Svc(SecurityContext(user_id="u", roles=["viewer"])).do()


def test_rbac_require_any_permission_no_context():
    r = RBACManager()

    @r.require_any_permission(["a:b"])
    def f(security_context=None):
        return "ok"

    with pytest.raises(PermissionError):
        f()


# ---------------------------------------------------------------------------
# SecurityContext + thread-local helpers
# ---------------------------------------------------------------------------


def test_security_context_permissions_and_roles():
    ctx = SecurityContext(
        user_id="u",
        roles=["analyst"],
        permissions={"vector:read", "graph:*"},
    )
    assert ctx.has_permission("vector:read") is True
    assert ctx.has_permission("graph:traverse") is True  # wildcard
    assert ctx.has_permission("document:read") is False
    assert ctx.has_role("analyst") is True
    assert ctx.has_role("admin") is False


def test_thread_local_set_get_clear():
    clear_security_context()
    assert get_current_security_context() is None
    ctx = SecurityContext(user_id="u")
    set_security_context(ctx)
    assert get_current_security_context() is ctx
    clear_security_context()
    assert get_current_security_context() is None
    # clearing again is harmless
    clear_security_context()


def test_security_context_manager_nested():
    clear_security_context()
    outer = SecurityContext(user_id="outer")
    inner = SecurityContext(user_id="inner")
    with security_context(outer) as c1:
        assert c1 is outer
        assert get_current_security_context() is outer
        with security_context(inner):
            assert get_current_security_context() is inner
        # restored to outer
        assert get_current_security_context() is outer
    # restored to None (no previous)
    assert get_current_security_context() is None


def test_security_context_manager_no_previous():
    clear_security_context()
    with security_context(SecurityContext(user_id="x")):
        pass
    assert get_current_security_context() is None


# ---------------------------------------------------------------------------
# AuditEvent
# ---------------------------------------------------------------------------


def test_audit_event_to_dict_and_json():
    ev = AuditEvent(
        event_id="id1",
        event_type=AuditEventType.DATA_ACCESS,
        timestamp=datetime.now(timezone.utc),
        user_id="u",
        tenant_id="t",
        action="read",
        resource_type="vector",
        resource_id="v1",
        outcome="success",
    )
    d = ev.to_dict()
    assert d["event_type"] == "data_access"
    assert d["resource_id"] == "v1"
    import json

    assert json.loads(ev.to_json())["event_id"] == "id1"


# ---------------------------------------------------------------------------
# AuditLogger
# ---------------------------------------------------------------------------


def test_audit_logger_basic_with_context():
    clear_security_context()
    ctx = SecurityContext(
        user_id="alice",
        tenant_id="t1",
        client_ip="1.2.3.4",
        user_agent="ua",
        request_id="rq",
        session_id="ss",
    )
    log = AuditLogger()
    received = []
    log.on_event(lambda e: received.append(e))
    with security_context(ctx):
        ev = log.log(AuditEventType.SECURITY, "act", "res", resource_id="r1")
    assert ev.user_id == "alice"
    assert ev.tenant_id == "t1"
    assert ev.client_ip == "1.2.3.4"
    assert received and received[0] is ev


def test_audit_logger_no_context_is_system():
    clear_security_context()
    log = AuditLogger()
    ev = log.log(AuditEventType.SYSTEM, "boot", "system")
    assert ev.user_id == "system"
    assert ev.tenant_id is None


def test_audit_logger_callback_exception_swallowed():
    log = AuditLogger()

    def bad(_e):
        raise RuntimeError("x")

    log.on_event(bad)
    ev = log.log(AuditEventType.SYSTEM, "a", "r")  # should not raise
    assert ev is not None


def test_audit_logger_helpers():
    log = AuditLogger()
    a = log.log_authentication("u", "password", "success", details={"k": "v"})
    assert a.event_type == AuditEventType.AUTHENTICATION
    assert a.action == "authenticate_password"

    z = log.log_authorization("vector:search", "v1", allowed=False)
    assert z.outcome == "denied"
    assert z.resource_type == "vector"

    d = log.log_data_access("read", "vector", "v1")
    assert d.event_type == AuditEventType.DATA_ACCESS


def test_audit_logger_file_write(tmp_path):
    f = tmp_path / "audit.log"
    log = AuditLogger(log_file=str(f))
    log.log(AuditEventType.SYSTEM, "a", "r")
    contents = f.read_text().strip()
    assert contents
    import json

    assert json.loads(contents)["action"] == "a"


def test_audit_logger_file_write_error(monkeypatch, caplog):
    log = AuditLogger(log_file="/nonexistent_dir_xyz/audit.log")
    # open() raises -> logged, not raised
    ev = log.log(AuditEventType.SYSTEM, "a", "r")
    assert ev is not None


def test_audit_logger_signing_chain():
    log = AuditLogger(enable_signing=True, signing_key=b"key")
    ev1 = log.log(AuditEventType.SECURITY, "a", "r")
    ev2 = log.log(AuditEventType.SECURITY, "b", "r")
    assert "chain_hash" in ev1.metadata
    assert "chain_hash" in ev2.metadata
    assert ev1.metadata["chain_hash"] != ev2.metadata["chain_hash"]


def test_audit_logger_signing_no_key():
    log = AuditLogger(enable_signing=True)
    ev = log.log(AuditEventType.SECURITY, "a", "r")
    assert "chain_hash" in ev.metadata


def test_audit_logger_remote_batch_flush(fake_requests):
    fake_requests["install"]([FakeResp(200, {})])
    log = AuditLogger(remote_endpoint="https://collector")
    log._batch_size = 2
    log.log(AuditEventType.SYSTEM, "a", "r")
    assert len(log._batch) == 1
    log.log(AuditEventType.SYSTEM, "b", "r")  # triggers flush at size 2
    assert log._batch == []
    assert fake_requests["post"].calls


def test_audit_logger_flush_empty():
    log = AuditLogger(remote_endpoint="https://collector")
    # no events -> early return, no error
    log._flush_batch()


def test_audit_logger_flush_error_swallowed(fake_requests):
    fake_requests["install"]([RuntimeError("net down")])
    log = AuditLogger(remote_endpoint="https://collector")
    log._batch_size = 1
    log.log(AuditEventType.SYSTEM, "a", "r")  # flush raises internally -> swallowed
    assert True


def test_audit_logger_flush_non_200_keeps_batch(fake_requests):
    fake_requests["install"]([FakeResp(503, {})])
    log = AuditLogger(remote_endpoint="https://collector")
    log._batch_size = 1
    log.log(AuditEventType.SYSTEM, "a", "r")
    # non-200 -> batch not cleared
    assert len(log._batch) == 1


# ---------------------------------------------------------------------------
# MTLSConfig
# ---------------------------------------------------------------------------


def test_mtls_validate_disabled():
    cfg = MTLSConfig(enabled=False, client_cert_path="/nope")
    assert cfg.validate() == []


def test_mtls_validate_missing_files():
    cfg = MTLSConfig(
        enabled=True,
        client_cert_path="/nope/cert.pem",
        client_key_path="/nope/key.pem",
        ca_cert_path="/nope/ca.pem",
    )
    issues = cfg.validate()
    assert len(issues) == 3


def test_mtls_create_ssl_context_default(monkeypatch):
    import ssl as _ssl

    created = {}

    class FakeCtx:
        def __init__(self):
            self.minimum_version = None
            self.check_hostname = None
            self.verify_mode = None

        def load_cert_chain(self, certfile, keyfile):
            created["cert"] = (certfile, keyfile)

        def load_verify_locations(self, cafile):
            created["ca"] = cafile

        def load_default_certs(self):
            created["default"] = True

        def set_ciphers(self, c):
            created["ciphers"] = c

    monkeypatch.setattr(_ssl, "SSLContext", lambda proto: FakeCtx())
    cfg = MTLSConfig(enabled=True, min_tls_version="TLSv1.3")
    ctx = cfg.create_ssl_context()
    assert ctx.minimum_version == _ssl.TLSVersion.TLSv1_3
    assert created.get("default") is True
    assert ctx.verify_mode == _ssl.CERT_REQUIRED


def test_mtls_create_ssl_context_with_certs_and_ciphers(monkeypatch):
    import ssl as _ssl

    created = {}

    class FakeCtx:
        def __init__(self):
            self.minimum_version = None
            self.check_hostname = None
            self.verify_mode = None

        def load_cert_chain(self, certfile, keyfile):
            created["cert"] = (certfile, keyfile)

        def load_verify_locations(self, cafile):
            created["ca"] = cafile

        def load_default_certs(self):
            created["default"] = True

        def set_ciphers(self, c):
            created["ciphers"] = c

    monkeypatch.setattr(_ssl, "SSLContext", lambda proto: FakeCtx())
    cfg = MTLSConfig(
        enabled=True,
        client_cert_path="c.pem",
        client_key_path="k.pem",
        ca_cert_path="ca.pem",
        allowed_ciphers=["AES256", "CHACHA20"],
        min_tls_version="UNKNOWN",  # falls back to TLSv1.2
    )
    ctx = cfg.create_ssl_context()
    assert created["cert"] == ("c.pem", "k.pem")
    assert created["ca"] == "ca.pem"
    assert created["ciphers"] == "AES256:CHACHA20"
    assert ctx.minimum_version == _ssl.TLSVersion.TLSv1_2


# ---------------------------------------------------------------------------
# SecurityManager
# ---------------------------------------------------------------------------


def test_security_manager_defaults():
    sm = SecurityManager()
    assert sm.oauth2 is None
    assert isinstance(sm.rbac, RBACManager)
    assert isinstance(sm.audit, AuditLogger)
    assert sm.mtls is None
    assert sm.get_ssl_context() is None


def test_security_manager_with_oauth2():
    sm = SecurityManager(oauth2_config=OAuth2Config(client_id="c"))
    assert isinstance(sm.oauth2, OAuth2TokenManager)


def test_security_manager_create_context():
    sm = SecurityManager()
    ctx = sm.create_context(
        user_id="u", tenant_id="t", roles=["developer"], client_ip="ip", user_agent="ua"
    )
    assert ctx.user_id == "u"
    assert ctx.tenant_id == "t"
    assert "vector:*" in ctx.permissions
    assert ctx.request_id


def test_security_manager_create_context_no_roles():
    sm = SecurityManager()
    ctx = sm.create_context(user_id="u")
    assert ctx.roles == []


def test_security_manager_check_permission():
    sm = SecurityManager()
    clear_security_context()
    assert sm.check_permission("vector:search") is False  # no context
    ctx = sm.create_context(user_id="u", roles=["analyst"])
    with security_context(ctx):
        assert sm.check_permission("vector:search") is True
        assert sm.check_permission("vector:delete") is False


def test_security_manager_audit_wired():
    log = AuditLogger()
    events = []
    log.on_event(lambda e: events.append(e))
    sm = SecurityManager(audit_logger=log)
    # checking a permission triggers rbac audit callback -> audit.log_authorization
    sm.rbac.check_permission(["analyst"], "vector:search")
    assert events
    assert events[-1].event_type == AuditEventType.AUTHORIZATION


def test_security_manager_require_permission_decorator():
    sm = SecurityManager()
    dec = sm.require_permission("vector:search")

    @dec
    def f(security_context=None):
        return "ok"

    assert f(security_context=sm.create_context(user_id="u", roles=["analyst"])) == "ok"


def test_security_manager_ssl_context():
    sm = SecurityManager(mtls_config=MTLSConfig(enabled=False))
    assert sm.get_ssl_context() is None


def test_security_manager_ssl_context_enabled(monkeypatch):
    cfg = MTLSConfig(enabled=True)
    sentinel = object()
    monkeypatch.setattr(cfg, "create_ssl_context", lambda: sentinel)
    sm = SecurityManager(mtls_config=cfg)
    assert sm.get_ssl_context() is sentinel


# ---------------------------------------------------------------------------
# Edge cases for remaining lines
# ---------------------------------------------------------------------------


def test_refresh_max_attempts_exceeded_fallthrough(fake_requests, monkeypatch):
    """All attempts return non-200/non-400 -> loop falls through to final raise."""
    monkeypatch.setattr(sec.time, "sleep", lambda *_a, **_k: None)
    fake_requests["install"](
        [FakeResp(503, {}), FakeResp(503, {}), FakeResp(503, {})]
    )
    m = _mgr(max_refresh_attempts=3)
    m._token = OAuth2TokenResponse(access_token="OLD", refresh_token="RT")
    with pytest.raises(OAuth2Error):
        m.refresh()


def test_rbac_inheritance_diamond_visited_guard():
    """Diamond inheritance exercises the already-visited short-circuit."""
    r = RBACManager(
        custom_roles={
            "top": RoleDefinition(name="top", permissions={"t:1"}),
            "left": RoleDefinition(name="left", permissions={"l:1"}, inherits=["top"]),
            "right": RoleDefinition(
                name="right", permissions={"r:1"}, inherits=["top"]
            ),
            "bottom": RoleDefinition(
                name="bottom", permissions={"b:1"}, inherits=["left", "right"]
            ),
        }
    )
    perms = r.get_effective_permissions(["bottom"])
    assert {"b:1", "l:1", "r:1", "t:1"} <= perms
