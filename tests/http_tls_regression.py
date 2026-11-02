"""Reject untrusted HTTPS on all CLI surfaces; requires OpenSSL and debug binaries.

Run: python3 tests/http_tls_regression.py
Uses temporary certificates and loopback only; never modifies the system trust store.
"""
from pathlib import Path
import ssl
import subprocess
import tempfile
import threading

from directory_upload_regression import Fixture


def verify(binary, surface, root, certificate, key):
    root.mkdir()
    fixture = Fixture(binary, surface, root)
    try:
        fixture.server.shutdown()
        fixture.worker.join()
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        fixture.server.socket = context.wrap_socket(fixture.server.socket, server_side=True)
        is_tls_rejected = threading.Event()
        certificate_alerts = []
        original_get_request = fixture.server.get_request

        def record_handshake_failure():
            try:
                return original_get_request()
            except ssl.SSLError as error:
                certificate_alerts.append(str(error.reason))
                is_tls_rejected.set()
                raise

        fixture.server.get_request = record_handshake_failure
        fixture.worker = threading.Thread(target=fixture.server.serve_forever, daemon=True)
        fixture.worker.start()
        config = root / "config"
        config.write_text(config.read_text().replace("http://127.0.0.1:", "https://127.0.0.1:"))
        result = fixture.invoke(["ls", fixture.destination()])
        assert result.returncode != 0, (binary, surface, result.stdout, result.stderr)
        assert is_tls_rejected.wait(2), "client must reach TLS and reject the certificate"
        # [Review Fix #TlsAlert] Protocol/cipher failures must not count as CA validation.
        assert any("CERTIFICATE" in reason or "UNKNOWN_CA" in reason
                   for reason in certificate_alerts), certificate_alerts
        assert not fixture.server.events, fixture.server.events
        print(f"PASS: {binary.name} {surface} rejects untrusted CA before HTTP")
    finally:
        fixture.close()


def main():
    with tempfile.TemporaryDirectory(prefix="reqwest-tls-rejection-") as directory:
        root = Path(directory)
        certificate, key = root / "certificate.pem", root / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-nodes",
                        "-days", "1", "-subj", "/CN=localhost", "-addext",
                        "subjectAltName=IP:127.0.0.1,DNS:localhost", "-out", str(certificate),
                        "-keyout", str(key)], check=True, capture_output=True, timeout=15)
        cases = [("ve-storage-uni-cli", surface) for surface in ("tos", "ve-tos", "ve-adrive")]
        cases += [("tos-cli", "tos"), ("ve-tos-cli", "ve-tos"), ("ve-adrive-cli", "ve-adrive")]
        for index, (name, surface) in enumerate(cases):
            verify((Path("target/debug") / name).resolve(), surface, root / str(index), certificate, key)


if __name__ == "__main__":
    main()
