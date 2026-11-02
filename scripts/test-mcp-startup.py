"""Qualify offline discovery, opt-in launch and connector exit after desktop stop/crash."""
import argparse
from contextlib import ExitStack
import hashlib
import json
import os
from pathlib import Path
import queue
import secrets
import shutil
import subprocess
import tempfile
import threading
import time
from mcp_test_support import stop_connector


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--artifacts", required=True, type=Path)
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("This check qualifies the Windows desktop launcher.")
    executable = args.executable.resolve(strict=True)
    args.artifacts.mkdir(parents=True, exist_ok=True)
    profile = Path(tempfile.mkdtemp(prefix="mcp-startup-", dir=args.artifacts.resolve()))
    state = profile / "Transmog"
    state.mkdir()
    identity = secrets.token_urlsafe(12)
    key = "tmk_" + identity + "_" + secrets.token_urlsafe(32)
    grant = dict(id=identity, revision=1, name="Isolated startup qualification",
                 permissions=["inspect"], scope={"kind": "hosts", "hosts": ["localhost"]},
                 createdAt=int(time.time()), expiresAt=int(time.time()) + 3600,
                 lastUsedAt=None, revoked=False)
    # This is an owned test fixture, never an administrative product endpoint.
    (state / "mcp-access.json").write_text(json.dumps(dict(
        version=1, enabled=True, records=[dict(grant=grant, verifier=list(
            hashlib.sha256(key.encode()).digest()))])), encoding="utf-8")
    environment = dict(os.environ, LOCALAPPDATA=str(profile), TRANSMOG_ACCESS_KEY=key)
    environment.pop("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", None)
    powershell = shutil.which("pwsh") or shutil.which("powershell")
    if not powershell:
        raise RuntimeError("PowerShell is required for owned native window cleanup.")
    hidden = subprocess.CREATE_NO_WINDOW

    def shell(source):
        return subprocess.run([powershell, "-NoProfile", "-Command", source],
                              check=True, capture_output=True, text=True,
                              creationflags=hidden).stdout.strip()

    def children(parent):
        result = shell(f"Get-CimInstance Win32_Process -Filter \"ParentProcessId = {parent} AND Name = 'transmog.exe'\" | Select-Object -ExpandProperty ProcessId")
        return [int(line) for line in result.splitlines() if line.strip().isdigit()]

    def alive(pid):
        return shell(f"[bool](Get-Process -Id {pid} -ErrorAction SilentlyContinue)") == "True"

    def close_owned(pid):
        helper = Path(__file__).resolve().with_name("close-desktop-probe-window.ps1")
        subprocess.run([powershell, "-NoProfile", "-File", str(helper),
                        "-ProbeProcessId", str(pid), "-WindowTitle", "Transmog"],
                       check=True, capture_output=True, creationflags=hidden)
        deadline = time.monotonic() + 20
        while alive(pid):
            if time.monotonic() > deadline:
                raise RuntimeError("The owned desktop did not close normally.")
            time.sleep(0.1)

    def cleanup_desktop(pid):
        if alive(pid):
            try:
                close_owned(pid)
            except Exception:
                # Recheck ownership before the forced fallback. A missing window
                # must not leave the fixture alive or target an unrelated process.
                if connector is not None and pid in children(connector.pid):
                    shell(f"Stop-Process -Id {pid} -Force -ErrorAction SilentlyContinue")
                raise

    if shell("@(Get-Process -Name transmog -ErrorAction SilentlyContinue).Count") != "0":
        raise RuntimeError("Close the existing Transmog instance before isolated startup qualification.")

    connector = None
    messages = None
    pending_messages = {}

    def read_messages(process, output):
        for line in process.stdout:
            try:
                output.put(json.loads(line))
            except json.JSONDecodeError:
                output.put({"invalidProtocolOutput": True})

    def send(message):
        connector.stdin.write(json.dumps(dict(jsonrpc="2.0", **message)) + "\n")
        connector.stdin.flush()

    def receive(identity, seconds):
        if identity in pending_messages:
            return pending_messages.pop(identity)
        deadline = time.monotonic() + seconds
        while True:
            message = messages.get(timeout=max(0.1, deadline - time.monotonic()))
            if message.get("invalidProtocolOutput"):
                raise RuntimeError("The MCP connector wrote non-protocol data to stdout.")
            if message.get("id") == identity:
                return message
            if "id" in message:
                pending_messages[message["id"]] = message
            if time.monotonic() >= deadline:
                raise RuntimeError("The MCP operation exceeded its qualification deadline.")

    owned = None
    instances = []
    shutdowns = []
    try:
        for operation, shutdown in enumerate(["normal", "crash", "normal", "connector-first"], start=2):
            connector = subprocess.Popen([str(executable), "mcp", "serve", "--start-app"],
                                         env=environment, stdin=subprocess.PIPE,
                                         stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                         text=True, encoding="utf-8", creationflags=hidden)
            messages = queue.Queue()
            pending_messages.clear()
            reader = threading.Thread(target=read_messages, args=(connector, messages), daemon=True)
            reader.start()
            send(dict(id=1, method="initialize", params=dict(protocolVersion="2025-11-25",
                      capabilities={}, clientInfo=dict(name="startup-qualification", version="1"))))
            initialized = receive(1, 10)
            assert "result" in initialized, "Offline MCP initialization failed"
            assert children(connector.pid) == [], "Discovery launched the desktop before explicit connect"
            send(dict(method="notifications/initialized"))
            send(dict(id=operation, method="tools/call", params=dict(name="transmog_connect", arguments={})))
            send(dict(id=operation + 100, method="tools/call", params=dict(name="transmog_connect", arguments={})))
            response = receive(operation, 45)
            concurrent = receive(operation + 100, 45)["result"]
            assert not concurrent.get("isError"), "Concurrent explicit connect failed"
            result = response["result"]
            assert not result.get("isError"), "Explicit connect failed to launch the authorized desktop"
            context = result["structuredContent"]
            assert concurrent["structuredContent"]["instanceId"] == context["instanceId"], "Concurrent connects used different desktops"
            assert context["connected"] and context["grant"]["id"] == identity
            assert context["proxy"]["lifecycle"] == "stopped", "Startup started collection"
            assert context["recordingActive"] is False, "Startup started recording"
            assert len(list(state.glob("mcp-lifetime-*.lock"))) == 1, "Stale lifetime lease was not reclaimed"
            instances.append(context["instanceId"])
            launched = children(connector.pid)
            assert len(launched) == 1, "The CLI did not launch one owned desktop"
            owned = launched[0]
            if shutdown == "connector-first":
                started = time.monotonic()
                connector.stdin.close()
                assert connector.wait(timeout=5) == 0, "Connector did not end on client EOF"
                assert alive(owned), "Ending the connector killed the independently launched desktop"
                seconds = round(time.monotonic() - started, 3)
                close_owned(owned)
            elif shutdown == "crash":
                # Kill only the desktop just launched by this owned connector.
                shell(f"Stop-Process -Id {owned} -Force -ErrorAction Stop")
                assert (state / "mcp-endpoint.json").exists(), "Crash did not leave stale discovery to test"
            else:
                close_owned(owned)
            owned = None
            if shutdown != "connector-first":
                started = time.monotonic()
                # Keep stdin open: the desktop lease must drive this shutdown.
                assert connector.wait(timeout=5) == 0, "Connector did not end with its desktop"
                seconds = round(time.monotonic() - started, 3)
            shutdowns.append(dict(kind=shutdown, seconds=seconds))
            connector.stdin.close()
            reader.join(timeout=2)
            connector.stdout.close()
        assert len(set(instances)) == 4, "Restart reused a workspace instance"
        assert key not in (state / "mcp-access.json").read_text(encoding="utf-8")
        report = dict(offlineInitialization=True, explicitLaunch=True, persistentKeyReuse=True,
                      freshRestartInstance=True, noImplicitCollection=True,
                      connectorShutdowns=shutdowns, concurrentConnect=True,
                      desktopSurvivesConnector=True, profile=str(profile))
        (args.artifacts / "startup-result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(json.dumps(report))
    finally:
        with ExitStack() as cleanup:
            if connector is not None:
                cleanup.callback(stop_connector, connector)
            # Also cover launch failure before a tool result identified the child.
            if owned is not None:
                cleanup.callback(cleanup_desktop, owned)
            pending = set(children(connector.pid)) if connector is not None else set()
            pending.discard(owned)
            for pid in pending:
                cleanup.callback(cleanup_desktop, pid)


if __name__ == "__main__":
    main()
