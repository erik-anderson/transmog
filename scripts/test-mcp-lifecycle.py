"""Verify MCP subprocess shutdown with real stdio pipes and owned processes only."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
from mcp_test_support import OwnedProcess, stop_connector


HIDDEN = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
INITIALIZE = dict(id=1, method="initialize", params=dict(
    protocolVersion="2025-11-25", capabilities={},
    clientInfo=dict(name="lifecycle-qualification", version="1")))


def send(process, message):
    process.stdin.write(json.dumps(dict(jsonrpc="2.0", **message)) + "\n")
    process.stdin.flush()


def read_line(process, seconds=5):
    result = queue.Queue()

    def read():
        result.put(process.stdout.readline())

    reader = threading.Thread(target=read, daemon=True)
    reader.start()
    line = result.get(timeout=seconds)
    reader.join(timeout=1)
    return json.loads(line) if line else None


def check_case(executable, environment, name):
    if name == "trace-initialization-timeout":
        environment = dict(environment, RUST_LOG="trace")
    process = subprocess.Popen([str(executable), "mcp", "serve"], env=environment,
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, encoding="utf-8",
                               creationflags=HIDDEN)
    try:
        started = time.monotonic()
        if name == "trace-initialization-timeout":
            # Pings are permitted before initialize. Fill stderr while proving
            # protocol progress, then leave stdin open for the fatal timeout.
            for identity in range(1, 101):
                send(process, dict(id=identity, method="ping", params={}))
                assert read_line(process)["id"] == identity
        elif name not in ("uninitialized-eof", "initialization-timeout", "partial-initialize-timeout"):
            send(process, INITIALIZE)
            assert "result" in read_line(process), "Offline initialization failed"
            send(process, dict(method="notifications/initialized"))
        elif name == "partial-initialize-timeout":
            process.stdin.write('{"jsonrpc":"2.0",')
            process.stdin.flush()
        if name in ("stdin-eof", "uninitialized-eof"):
            process.stdin.close()
        elif name == "stdout-closed-input-open":
            # The one-line reader has returned; no thread owns stdout now.
            process.stdout.close()
            send(process, dict(id=2, method="tools/list", params={}))
        deadline = 34 if name.endswith("timeout") else 5
        code = process.wait(timeout=deadline)
        expected = 2 if name.endswith("timeout") else 0
        assert code == expected, f"{name}: exit {code}: {process.stderr.read()}"
        if name.endswith("timeout") and name != "trace-initialization-timeout":
            assert "did not initialize" in process.stderr.read()
        return dict(case=name, exitCode=code, seconds=round(time.monotonic() - started, 3))
    finally:
        stop_connector(process)


def check_parent_exit(executable, environment, directory):
    if not OwnedProcess.supported():
        return dict(case="client-process-exit-input-open", skipped="Stable process handles are unavailable on this platform.")
    # The outer test keeps the input writer open after killing the intermediary
    # MCP client. EOF cannot explain the server's exit in this scenario.
    pid_file = directory / "owned-connector.pid"
    pid_file.unlink(missing_ok=True)
    source = """import pathlib, subprocess, sys
child = subprocess.Popen([sys.argv[1], 'mcp', 'serve'], stdin=sys.stdin,
                         stdout=sys.stdout, stderr=sys.stderr)
try:
    pathlib.Path(sys.argv[2]).write_text(str(child.pid), encoding='utf-8')
except BaseException:
    child.kill()
    child.wait()
    raise
child.wait()
"""
    client = subprocess.Popen([sys.executable, "-c", source, str(executable), str(pid_file)],
                              env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True, encoding="utf-8",
                              creationflags=HIDDEN)
    connector_pid = None
    connector = None
    stopped = False
    try:
        deadline = time.monotonic() + 5
        while connector_pid is None:
            try:
                connector_pid = int(pid_file.read_text(encoding="utf-8"))
            except (FileNotFoundError, ValueError):
                if time.monotonic() >= deadline:
                    raise RuntimeError("The owned MCP client did not report its child process.")
                time.sleep(0.01)
        connector = OwnedProcess(connector_pid)
        send(client, INITIALIZE)
        assert "result" in read_line(client)
        send(client, dict(method="notifications/initialized"))
        started = time.monotonic()
        client.kill()
        # Leave the client unreaped until its connector ends. On Unix it is a
        # zombie: still present in process listings, but no longer a live owner.
        assert read_line(client, seconds=5) is None, "Connector survived its client"
        assert connector.wait(5), "Connector closed output but remained running"
        stopped = True
        return dict(case="client-process-exit-input-open", seconds=round(time.monotonic() - started, 3))
    finally:
        try:
            if connector is not None and not stopped:
                connector.kill()
                assert connector.wait(5), "Owned connector could not be stopped during cleanup"
        finally:
            try:
                stop_connector(client)
            finally:
                if connector is not None:
                    connector.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--artifacts", required=True, type=Path)
    parser.add_argument("--cycles", type=int, default=5)
    args = parser.parse_args()
    if args.cycles < 1:
        parser.error("--cycles must be positive")
    executable = args.executable.resolve(strict=True)
    args.artifacts.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="mcp-lifecycle-", dir=args.artifacts.resolve()) as temporary:
        profile = Path(temporary)
        environment = dict(os.environ, LOCALAPPDATA=str(profile), XDG_STATE_HOME=str(profile))
        environment.pop("TRANSMOG_ACCESS_KEY", None)
        cases = []
        for _ in range(args.cycles):
            for name in ("uninitialized-eof", "stdin-eof", "stdout-closed-input-open"):
                cases.append(check_case(executable, environment, name))
            cases.append(check_parent_exit(executable, environment, profile))
        # Missing and partial initialization must end with input held open.
        with ThreadPoolExecutor(max_workers=3) as executor:
            futures = [executor.submit(check_case, executable, environment, name)
                       for name in ("initialization-timeout", "partial-initialize-timeout", "trace-initialization-timeout")]
            cases.extend(future.result() for future in futures)
    report = dict(executable=str(executable), cycles=args.cycles, cases=cases,
                  existingProcessesTerminated=False)
    (args.artifacts / "lifecycle-result.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
