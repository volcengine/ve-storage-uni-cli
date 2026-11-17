"""Validate frozen design contracts, fixtures and real bootstrap HTTP responses.

Install tests/requirements.txt and build `cargo build -p ask-auth --offline` first.
Run this file with Python; it needs only an isolated loopback listener.
"""

import base64
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time
import unittest
import urllib.error
import urllib.request
import uuid

from jsonschema import Draft202012Validator, FormatChecker
from openapi_spec_validator import validate as validate_openapi

SERVICE_ROOT = Path(__file__).resolve().parents[1]
WORKSPACE_ROOT = SERVICE_ROOT.parents[1]
CONTRACT_ROOT = SERVICE_ROOT / "contracts"
OPENAPI = json.loads((CONTRACT_ROOT / "ask-auth-v1.openapi.json").read_text())
FIXTURES = json.loads((CONTRACT_ROOT / "ask-auth-v1.15.fixtures.json").read_text())
AUTH_ID = FIXTURES["creation"]["auth_id"]
INVALID_UUIDS = [AUTH_ID.replace("7111", "4111"), AUTH_ID.replace("7111-8", "7111-7"),
                 AUTH_ID.replace("-", ""), "AAAAAAAA-AAAA-7AAA-8AAA-AAAAAAAAAAAA"]


def schema_validator(name):
    """Resolve a named schema using only checked-in components."""
    schema = {"$ref": f"#/components/schemas/{name}", "components": OPENAPI["components"]}
    return Draft202012Validator(schema, format_checker=FormatChecker())


class FrozenContracts(unittest.TestCase):
    def test_openapi_and_json_schemas_are_valid(self):
        validate_openapi(OPENAPI)
        self.assertEqual(len(OPENAPI["paths"]), 9)
        self.assertEqual(len(OPENAPI["components"]["schemas"]), 22)
        for schema in OPENAPI["components"]["schemas"].values():
            Draft202012Validator.check_schema(schema)
        result_schema = json.loads((CONTRACT_ROOT / "ask-result-v1.schema.json").read_text())
        Draft202012Validator.check_schema(result_schema)

    def test_sanitized_fixtures_match_frozen_request_and_bundle_schemas(self):
        self.assertEqual(FIXTURES["design_version"], "v1.15")
        cases = {"creation": "LoginCreateRequest", "claim": "ClaimRequest",
                 "refresh": "RefreshRequest", "logout": "LogoutRequest",
                 "initial_token_bundle": "TokenBundle"}
        for fixture_name, schema_name in cases.items():
            with self.subTest(fixture=fixture_name):
                schema_validator(schema_name).validate(FIXTURES[fixture_name])
        schema_validator("AuthId").validate(FIXTURES["status"]["query"]["auth_id"])
        schema_validator("Nonce").validate(FIXTURES["status"]["broker_token"])
        operation_ids = [FIXTURES["trace_request_id"], FIXTURES["claim_headers"]["Idempotency-Key"],
                         FIXTURES["refresh_headers"]["Idempotency-Key"]]
        for operation_id in operation_ids:
            schema_validator("Uuid").validate(operation_id)
        self.assertEqual(len(set([AUTH_ID, *operation_ids, FIXTURES["ask_result"]["request_id"]])), 5)

    def test_frozen_schemas_reject_aliases_unknown_fields_and_noncanonical_values(self):
        logout_validator = schema_validator("LogoutRequest")
        for payload in [{"session_id": AUTH_ID}, {"auth_id": AUTH_ID, "user_id": "untrusted"}]:
            self.assertFalse(logout_validator.is_valid(payload))
        for nonce in ["A" * 42 + "B", "A" * 43 + "="]:
            self.assertFalse(schema_validator("Nonce").is_valid(nonce))
        schema_validator("Uuid").validate(AUTH_ID)
        for name in ["Uuid", "AuthId"]:
            for invalid_id in INVALID_UUIDS:
                self.assertFalse(schema_validator(name).is_valid(invalid_id))
        result_schema = json.loads((CONTRACT_ROOT / "ask-result-v1.schema.json").read_text())
        validator = Draft202012Validator(result_schema, format_checker=FormatChecker())
        payload = FIXTURES["ask_result"]
        validator.validate(payload)
        for invalid_id in INVALID_UUIDS:
            self.assertFalse(validator.is_valid({**payload, "request_id": invalid_id}))


class LiveBootstrap(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        binary = Path(os.environ.get("ASK_AUTH_TEST_BINARY", WORKSPACE_ROOT / "target/debug/ask-auth"))
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        environment = {key: value for key, value in os.environ.items() if not key.startswith("ASK_AUTH_")}
        environment.update(ASK_AUTH_LISTEN=f"127.0.0.1:{port}", ASK_AUTH_LOG_LEVEL="off")
        cls.origin = f"http://127.0.0.1:{port}"
        cls.process = subprocess.Popen([str(binary)], env=environment,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        cls.addClassCleanup(cls.stop_process)
        cls.wait_for_health()

    @classmethod
    def wait_for_health(cls):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if cls.process.poll() is not None:
                raise RuntimeError("bootstrap exited before health check")
            try:
                with urllib.request.urlopen(cls.origin + "/healthz", timeout=0.2) as response:
                    if response.status == 200:
                        return
            except (OSError, urllib.error.URLError):
                time.sleep(0.02)
        raise RuntimeError("bootstrap did not become healthy within five seconds")

    @classmethod
    def stop_process(cls):
        if cls.process.poll() is None:
            cls.process.send_signal(signal.SIGTERM)
        try:
            cls.process.communicate(timeout=6)
        except subprocess.TimeoutExpired:
            cls.process.kill()
            cls.process.communicate(timeout=2)
            raise AssertionError("bootstrap failed to shut down gracefully")
        if cls.process.returncode != 0:
            raise AssertionError("bootstrap exited unsuccessfully")

    def call(self, method, path, payload=None, headers=None):
        body = None if payload is None else json.dumps(payload).encode()
        request_headers = {"Content-Type": "application/json", "X-Request-ID": AUTH_ID}
        request_headers.update(headers or {})
        request = urllib.request.Request(self.origin + path, data=body,
                                         method=method, headers=request_headers)
        try:
            response = urllib.request.urlopen(request, timeout=2)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            return response.status, response.headers, response.read()

    def validate_json_response(self, observed, expected_status, schema_name):
        status, headers, body = observed
        self.assertEqual(status, expected_status)
        self.assertEqual(headers["Cache-Control"], "no-store")
        request_id = headers["X-Request-ID"]
        schema_validator("Uuid").validate(request_id)
        self.assertNotEqual(request_id, AUTH_ID)
        payload = json.loads(body)
        schema_validator(schema_name).validate(payload)
        if "request_id" in payload:
            self.assertEqual(payload["request_id"], request_id)
        return payload

    def test_health_and_readiness_match_frozen_schema(self):
        for path, status, state in [("/healthz", 200, "ok"), ("/readyz", 503, "unavailable")]:
            payload = self.validate_json_response(self.call("GET", path), status, "HealthResponse")
            self.assertEqual(payload, {"status": state})

    def test_all_business_routes_fail_closed_with_contract_errors(self):
        broker = {"Authorization": f"Bearer {FIXTURES['status']['broker_token']}"}
        ticket = {"Authorization": f"Bearer {FIXTURES['creation']['login_ticket']}"}
        claim_headers = {**ticket, **FIXTURES["claim_headers"]}
        refresh_headers = {**broker, **FIXTURES["refresh_headers"]}
        cases = [("POST", "/v1/auth/login-attempts", FIXTURES["creation"], {}),
                 ("GET", f"/v1/auth/login-attempts/{AUTH_ID}", None, ticket),
                 ("POST", "/v1/auth/token", FIXTURES["claim"], claim_headers),
                 ("POST", "/v1/auth/refresh", FIXTURES["refresh"], refresh_headers),
                 ("GET", f"/v1/auth/status?auth_id={AUTH_ID}", None, broker),
                 ("POST", "/v1/auth/logout", FIXTURES["logout"], broker)]
        for method, path, payload, headers in cases:
            with self.subTest(path=path):
                result = self.validate_json_response(self.call(method, path, payload, headers), 503, "ErrorResponse")
                self.assertEqual(result["error"]["code"], "upstream_unavailable")

    def test_invalid_requests_and_missing_proofs_have_frozen_error_shapes(self):
        cases = [("POST", "/v1/auth/login-attempts", {}, 400, "invalid_request"),
                 ("GET", f"/v1/auth/status?auth_id={AUTH_ID}", None, 401, "authentication_required"),
                 ("GET", "/v1/auth/status", None, 400, "invalid_request"),
                 ("GET", "/ask/proxy", None, 404, "invalid_request"),
                 ("POST", "/healthz", None, 405, "invalid_request")]
        for method, path, payload, status, code in cases:
            result = self.validate_json_response(self.call(method, path, payload), status, "ErrorResponse")
            self.assertEqual(result["error"]["code"], code)

    def test_callback_is_fixed_html_with_no_reflection(self):
        state = base64.urlsafe_b64encode(uuid.UUID(AUTH_ID).bytes + bytes(32)).decode().rstrip("=")
        for query, expected in [(f"state={state}&code=SECRET_CALLBACK_SENTINEL", 503),
                                ("state=bad&code=SECRET_CALLBACK_SENTINEL", 400)]:
            status, headers, body = self.call("GET", "/v1/oauth/doubao/callback?" + query)
            self.assertEqual(status, expected)
            self.assertEqual(headers["Cache-Control"], "no-store")
            self.assertEqual(headers["Referrer-Policy"], "no-referrer")
            schema_validator("Uuid").validate(headers["X-Request-ID"])
            self.assertNotIn(b"SECRET_CALLBACK_SENTINEL", body)
            self.assertNotIn(state.encode(), body)


if __name__ == "__main__":
    unittest.main()
