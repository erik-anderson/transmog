"""Qualify process inspection against real CLI processes and pinned loopback TLS.

Every run uses manual proxy routing and isolated state. No CA is installed.
Windows console signals target only a hidden console created by this script.
"""
import argparse
import contextlib
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import signal
import socket
import ssl
import subprocess
import sys
import threading
import time
import uuid


def stop_owned(process):
    if process.poll() is not None:
        return
    if os.name == "nt":
        helper = """import ctypes,sys
k=ctypes.WinDLL('kernel32',use_last_error=True)
k.FreeConsole()
if not k.AttachConsole(int(sys.argv[1])): raise ctypes.WinError(ctypes.get_last_error())
try:
 if not k.SetConsoleCtrlHandler(None,True): raise ctypes.WinError(ctypes.get_last_error())
 if not k.GenerateConsoleCtrlEvent(0,0): raise ctypes.WinError(ctypes.get_last_error())
finally: k.FreeConsole()
"""
        subprocess.run([sys.executable, "-c", helper, str(process.pid)], check=True, timeout=10)
    else:
        process.send_signal(signal.SIGINT)
    process.wait(timeout=30)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform not in ("win32", "linux"):
        parser.error("Process-name inspection qualification requires Windows or Linux.")
    executable = args.executable.resolve(strict=True)
    root = args.artifacts.resolve() / ("process-inspection-" + uuid.uuid4().hex)
    root.mkdir(parents=True)
    process_name = Path(sys.executable).resolve().name
    profile = root / "profile"
    environment = dict(os.environ, LOCALAPPDATA=str(profile), XDG_STATE_HOME=str(profile),
                       XDG_DATA_HOME=str(profile))
    body = b"process inspection fixture"
    observed = []
    lock = threading.Lock()

    def command(*options, ok=True):
        result = subprocess.run([str(executable), *map(str, options)], env=environment,
                                stdin=subprocess.DEVNULL, capture_output=True,
                                text=True, encoding="utf-8", errors="replace", timeout=45)
        if (result.returncode == 0) != ok:
            raise AssertionError((options, result.stdout, result.stderr))
        return result.stdout + result.stderr

    # Rejections must precede output preparation and preference/root writes.
    command("record", "--output", root / "invalid.tmcap", "--process", process_name,
            "--exclude-process", process_name, ok=False)
    command("record", "--output", root / "invalid.tmcap", "--no-decrypt-https",
            "--install-root", ok=False)
    command("serve", "--no-decrypt-https", "--ca-key", root / "absent.key", ok=False)
    assert not profile.exists() and not (root / "invalid.tmcap").exists()

    ca, key = root / "ca.pem", root / "ca.key"
    certificate, leaf_key = root / "origin.pem", root / "origin.key"
    command("ca", "generate", "--cert", ca, "--key", key)
    command("ca", "issue", "--ca-cert", ca, "--ca-key", key, "--identity", "127.0.0.1",
            "--cert", certificate, "--key", leaf_key, "--days", "1")
    expected_certificate = ssl.PEM_cert_to_DER_cert(certificate.read_text(encoding="utf-8"))

    class Origin(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self):
            with lock:
                observed.append(self.headers.get("x-intercept-test"))
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    plain = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    secure = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(certificate, leaf_key)
    secure.socket = tls.wrap_socket(secure.socket, server_side=True)
    for server in (plain, secure):
        threading.Thread(target=server.serve_forever, daemon=True).start()

    @contextlib.contextmanager
    def running(name, options, ready_pattern=r"LISTEN_ADDR=([^\r\n]+)"):
        log_path = root / (name + ".log")
        startup = {}
        if os.name == "nt":
            info = subprocess.STARTUPINFO()
            info.dwFlags |= subprocess.STARTF_USESHOWWINDOW
            info.wShowWindow = subprocess.SW_HIDE
            startup = dict(startupinfo=info, creationflags=subprocess.CREATE_NEW_CONSOLE)
        with log_path.open("wb") as log:
            process = subprocess.Popen([str(executable), *map(str, options)], env=environment,
                                       stdin=subprocess.DEVNULL, stdout=log,
                                       stderr=subprocess.STDOUT, **startup)
            try:
                deadline = time.monotonic() + 45
                while time.monotonic() < deadline:
                    text = log_path.read_text(encoding="utf-8", errors="replace")
                    match = re.search(ready_pattern, text)
                    if match:
                        host, port = match.group(1).rsplit(":", 1)
                        yield host, int(port)
                        stop_owned(process)
                        assert process.returncode == 0, log_path.read_text(encoding="utf-8")
                        break
                    if process.poll() is not None:
                        raise AssertionError(text)
                    time.sleep(.05)
                else:
                    raise AssertionError("CLI did not become ready: " + str(log_path))
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)

    def http_request(endpoint, expected_marker):
        connection = http.client.HTTPConnection(*endpoint, timeout=10)
        try:
            connection.request("GET", f"http://127.0.0.1:{plain.server_port}/plain")
            response = connection.getresponse()
            assert response.status == 200 and response.read() == body
        finally:
            connection.close()
        with lock:
            assert observed[-1] == expected_marker, observed

    def tls_request(endpoint, inspected=False):
        with socket.create_connection(endpoint, timeout=10) as raw:
            authority = f"127.0.0.1:{secure.server_port}"
            raw.sendall(f"CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n".encode())
            response = bytearray()
            while not response.endswith(b"\r\n\r\n"):
                chunk = raw.recv(1)
                assert chunk and len(response) < 16_384, response
                response.extend(chunk)
            assert response.startswith(b"HTTP/1.1 200"), response
            context = ssl.create_default_context(cafile=str(ca))
            with context.wrap_socket(raw, server_hostname="127.0.0.1") as client:
                actual_certificate = client.getpeercert(binary_form=True)
                if inspected:
                    assert actual_certificate != expected_certificate, "Selected HTTPS did not enter inspection"
                else:
                    assert actual_certificate == expected_certificate, "TLS certificate pin changed"
                client.sendall(f"GET /pinned HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n".encode())
                response = http.client.HTTPResponse(client)
                response.begin()
                assert response.status == 200 and response.read() == body
        with lock:
            expected_marker = "process-fixture" if inspected else None
            assert observed[-1] == expected_marker, "HTTPS inspection rules did not match the process policy"

    def check_capture(path, expected_exchanges):
        summary = command("capture", "inspect", "--input", path)
        assert "SEALED=true" in summary, summary
        assert int(re.search(r"EXCHANGES=(\d+)", summary).group(1)) == expected_exchanges, summary
        command("capture", "validate", "--input", path)

    results = {}
    try:
        for name, options, capture_count, inspect_tls, marker in (
            ("all-inspected", [], 2, True, "process-fixture"),
            ("all-tunnels", ["--no-decrypt-https"], 2, False, "process-fixture"),
            ("except-pinned", ["--exclude-process", process_name], 0, False, None),
            ("only-current", ["--process", process_name], 2, True, "process-fixture"),
            ("only-other", ["--process", "transmog-nonexistent-fixture.exe"], 0, False, None),
        ):
            capture = root / (name + ".tmcap")
            credentials = [] if "--no-decrypt-https" in options else ["--ca-cert", ca, "--ca-key", key, "--upstream-ca-cert", ca]
            with running(name, ["serve", *credentials, *options, "--capture", capture,
                                "--capture-bodies", "--proof-id", "process-fixture"]) as endpoint:
                http_request(endpoint, marker)
                tls_request(endpoint, inspected=inspect_tls)
            check_capture(capture, capture_count)
            results[name] = "passed"

        capture = root / "record-tunnels.tmcap"
        with running("record-tunnels", ["record", "--output", capture, "--no-decrypt-https",
                     "--no-system-proxy", "--process", process_name],
                     r"Proxy address: ([^\r\n]+)") as endpoint:
            http_request(endpoint, None)
            tls_request(endpoint)
        check_capture(capture, 2)
        assert not list(profile.rglob("root-*.json")), "Certificate-free run created root ownership"
        assert not list(profile.rglob("*.key")) and not list(profile.rglob("*.pem"))
        results["record-without-ca"] = "passed"
        results["invalid-options-before-writes"] = "passed"
        (root / "results.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(dict(artifacts=str(root), results=results), indent=2))
    finally:
        for server in (plain, secure):
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
