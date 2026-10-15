"""Qualify copied/linked files through the real proxy and a controlled local origin."""
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from repo_tools import ROOT, cargo_output, command, development_environment, executable, main_guard, native_name


class DenyOrigin(BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.requests += 1
        self.send_error(502, "Auto-responses must not contact the origin")

    def log_message(self, format, *args):
        pass


def main():
    env = development_environment()
    output = cargo_output(env) / "debug"
    command(["cargo", "build", "--locked", "-p", "transmog", "-p", "transmog-script-host", "-p", "transmog-preview-worker"], env=env)
    evidence = ROOT / "artifacts"
    evidence.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="response-file-wire-", dir=evidence) as directory:
        run = Path(directory)
        ca, key = run / "ca.pem", run / "ca.key"
        command([output / native_name("transmog-cli"), "ca", "generate", "--cert", ca, "--key", key, "--name", "Response file qualification"], env=env)
        origin = HTTPServer(("127.0.0.1", 0), DenyOrigin)
        origin.requests = 0
        worker = threading.Thread(target=origin.serve_forever, daemon=True)
        worker.start()
        env.update(
            TRANSMOG_CURL=str(executable("curl", env)),
            TRANSMOG_NGINX_URL=f"http://127.0.0.1:{origin.server_port}/",
            TRANSMOG_TEST_CA_CERT=str(ca), TRANSMOG_TEST_CA_KEY=str(key),
            TRANSMOG_SCRIPT_HOST=str(output / native_name("transmog-script-host")),
            TRANSMOG_PREVIEW_WORKER=str(output / native_name("transmog-preview-worker")),
        )
        try:
            for test in ("linked_response_streams_compression_and_recovers_after_a_missing_file", "explicit_file_copy_above_old_cutoff_serves_saved_bytes", "linked_snapshot_history_survives_external_edits_and_native_save"):
                command(["cargo", "test", "--locked", "-p", "transmog-app", "--test", "headless_product", test, "--", "--ignored", "--nocapture"], env=env)
            if origin.requests:
                raise RuntimeError(f"Auto-response rules contacted the origin {origin.requests} times")
            print("Live copied/linked files: zero origin requests verified.")
        finally:
            origin.shutdown()
            origin.server_close()
            worker.join()


if __name__ == "__main__":
    main_guard(main)
