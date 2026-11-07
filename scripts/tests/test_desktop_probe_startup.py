"""DevTools availability is not application-page readiness."""
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import unittest

SHELL = shutil.which("pwsh") or shutil.which("powershell")
SCRIPT = Path(__file__).resolve().parents[1] / "desktop-probe-startup.ps1"
PAGE = {"type": "page", "title": "Transmog", "url": "http://transmog-ui.localhost/",
        "webSocketDebuggerUrl": "ws://127.0.0.1/devtools/page/owned"}


@contextmanager
def discovery(responses):
    requests = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            response = responses[min(len(requests) - 1, len(responses) - 1)]
            body = json.dumps(response).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server.server_port, requests
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


@unittest.skipUnless(SHELL, "PowerShell is required for the native probe startup fixture")
class DesktopProbeStartupTests(unittest.TestCase):
    def probe(self, root, port, *, exited=False, discover=False):
        fixture = Path(root) / "probe.ps1"
        fixture.write_text("""param([string]$Helper, [string]$Root, [int]$Port, [int]$Exited, [int]$Discover)
$ErrorActionPreference = 'Stop'
. $Helper
$result = Wait-DesktopProbePage -Process ([pscustomobject]@{HasExited=[bool]$Exited}) -ProfileRoot $Root -Port $Port -DiscoverPort:([bool]$Discover) -TimeoutSeconds 1
$result | ConvertTo-Json -Depth 6 -Compress
""")
        result = subprocess.run([SHELL, "-NoProfile", "-File", str(fixture), str(SCRIPT), str(root),
                                 str(port), str(int(exited)), str(int(discover))],
                                check=True, capture_output=True, text=True, timeout=15)
        return json.loads(result.stdout)

    def test_waits_through_workers_blank_pages_and_title_initialization(self):
        initial = [{"type": "worker", "title": "Transmog"},
                   dict(PAGE, title="", url="about:blank")]
        # The old wrapper accepted this nonempty list; the smoke found no page.
        self.assertTrue(initial)
        self.assertFalse(any(item["type"] == "page" and item["title"] == "Transmog" for item in initial))
        with tempfile.TemporaryDirectory() as root, discovery([initial, [dict(PAGE, title="")], [PAGE]]) as (port, requests):
            result = self.probe(root, port)
        self.assertTrue(result["Ready"])
        self.assertEqual(result["Attempts"], 3)
        self.assertEqual(requests, ["/json/list"] * 3)
        self.assertEqual(result["Targets"], [PAGE])

    def test_duplicate_app_pages_fail_closed(self):
        with tempfile.TemporaryDirectory() as root, discovery([[PAGE, dict(PAGE)]]) as (port, requests):
            result = self.probe(root, port)
        self.assertFalse(result["Ready"])
        self.assertEqual(result["Reason"], "ambiguous-pages")
        self.assertEqual(len(requests), 1)

    def test_exited_owned_process_does_not_accept_another_page(self):
        with tempfile.TemporaryDirectory() as root, discovery([[PAGE]]) as (port, requests):
            result = self.probe(root, port, exited=True)
        self.assertFalse(result["Ready"])
        self.assertEqual(result["Reason"], "process-exited")
        self.assertFalse(requests)

    def test_blank_or_unconnectable_pages_exhaust_the_existing_deadline(self):
        for target in (dict(PAGE, title=""), dict(PAGE, type="worker"), dict(PAGE, webSocketDebuggerUrl="")):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as root, discovery([[target]]) as (port, _):
                result = self.probe(root, port)
            self.assertFalse(result["Ready"])
            self.assertEqual(result["Reason"], "deadline")
            self.assertEqual(result["Targets"], [target])

    def test_viewer_profile_discovers_its_ephemeral_port(self):
        with tempfile.TemporaryDirectory() as root, discovery([[PAGE]]) as (port, _):
            profile = Path(root) / "WebView2"
            profile.mkdir()
            (profile / "DevToolsActivePort").write_text(str(port) + "\n/devtools/browser/owned\n")
            result = self.probe(root, 0, discover=True)
        self.assertTrue(result["Ready"])
        self.assertEqual(result["Port"], port)


if __name__ == "__main__":
    unittest.main()
