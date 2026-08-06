from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("mvp_smoke", ROOT / "scripts/mvp_smoke.py")
assert SPEC and SPEC.loader
MVP_SMOKE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MVP_SMOKE
SPEC.loader.exec_module(MVP_SMOKE)


class FakeResponse:
    def __init__(self, status: int, payload: dict) -> None:
        self.status = status
        self.payload = json.dumps(payload).encode()

    def __enter__(self):
        return self

    def __exit__(self, *_args) -> None:
        return None

    def read(self) -> bytes:
        return self.payload


class Responder:
    def __init__(self, leak: bool = False) -> None:
        self.leak = leak
        self.deleted = False

    def __call__(self, request, **_kwargs):
        path = request.full_url.split("example.test", 1)[-1]
        method = request.get_method()
        if path == "/health":
            return FakeResponse(200, {"status": "healthy"})
        if method == "POST" and path == "/api/v2/collections":
            return FakeResponse(201, {"created": True})
        if method == "POST" and path.endswith("/records/batch"):
            return FakeResponse(200, {"inserted_count": 3})
        if method == "POST" and path.endswith("/records/scan"):
            return FakeResponse(
                200, {"records": [{"id": "runbook-api"}, {"id": "incident-api"}]}
            )
        if method == "POST" and path.endswith("/search"):
            if self.leak:
                matches = [{"id": "runbook-data"}]
            elif self.deleted:
                matches = [{"id": "incident-api"}]
            else:
                matches = [{"id": "runbook-api"}, {"id": "incident-api"}]
            return FakeResponse(200, {"matches": matches})
        if method == "GET" and path.endswith("/records/runbook-api"):
            return FakeResponse(200, {"id": "runbook-api"})
        if method == "GET" and path.endswith("/route-health"):
            return FakeResponse(
                200,
                {
                    "writes": {
                        "conditional_write": False,
                        "filter_write": False,
                        "patch": False,
                    },
                    "freshness": {},
                    "filtered_ann": {},
                    "object_economy": {},
                    "recall_probe": {},
                },
            )
        if method == "DELETE" and path.endswith("/records/runbook-api"):
            self.deleted = True
            return FakeResponse(200, {"deleted": True})
        raise AssertionError(f"unexpected request: {method} {path}")


class SmokeTest(unittest.TestCase):
    def test_complete_corridor_roundtrip(self) -> None:
        with patch("urllib.request.urlopen", side_effect=Responder()):
            report = MVP_SMOKE.run_smoke("http://example.test", timeout=2)
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["canonical_api"], "REST /api/v2")
        self.assertEqual(report["engine"], "sst")
        self.assertGreaterEqual(report["step_count"], 8)

    def test_filter_leak_is_a_hard_failure(self) -> None:
        with patch("urllib.request.urlopen", side_effect=Responder(leak=True)):
            with self.assertRaises(MVP_SMOKE.SmokeFailure):
                MVP_SMOKE.run_smoke("http://example.test", timeout=0.01)


if __name__ == "__main__":
    unittest.main()
