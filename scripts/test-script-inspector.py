"""Verify the pinned V8 Inspector in an owned Windows AppContainer probe."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import queue
import struct
import subprocess
import threading
import time

from repo_tools import ROOT, main_guard

FRAME_BYTES = 64 * 1024


class Probe:
    def __init__(self, executable: Path):
        self.process = subprocess.Popen(
            [str(executable)], cwd=executable.parent, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        self.events = queue.Queue(maxsize=256)
        self.transcript = []
        self.next_id = 0
        self.deadline = time.monotonic() + 20
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()

    def read(self):
        try:
            while True:
                header = self.exact(4)
                length, = struct.unpack("<I", header)
                if not 0 < length <= FRAME_BYTES:
                    raise ValueError(f"Invalid frame length {length}")
                self.events.put(json.loads(self.exact(length)), timeout=2)
        except (EOFError, OSError, ValueError, queue.Full) as error:
            try:
                self.events.put(error, timeout=2)
            except queue.Full:
                pass

    def exact(self, size):
        value = bytearray()
        while len(value) < size:
            part = self.process.stdout.read(size - len(value))
            if not part:
                raise EOFError("Probe pipe closed")
            value.extend(part)
        return value

    def send(self, value):
        body = json.dumps(value).encode()
        assert len(body) <= FRAME_BYTES
        self.process.stdin.write(struct.pack("<I", len(body)) + body)
        self.process.stdin.flush()

    def until(self, predicate):
        while True:
            timeout = min(5, self.deadline - time.monotonic())
            if timeout <= 0:
                raise TimeoutError("Inspector proof exceeded total deadline")
            value = self.events.get(timeout=timeout)
            if isinstance(value, Exception):
                raise value
            if len(self.transcript) >= 256:
                raise ValueError("Inspector proof emitted too many events")
            self.transcript.append(value)
            if predicate(value):
                return value

    def event(self, name):
        return self.until(lambda value: value.get("event") == name or value.get("method") == name)

    def command(self, method, params=None, *, error=False):
        self.next_id += 1
        self.send({"id": self.next_id, "method": method, "params": params or {}})
        reply = self.until(lambda value: value.get("id") == self.next_id)
        assert ("error" in reply) == error, reply
        return reply.get("result", reply.get("error"))

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
        self.process.wait(timeout=5)
        self.reader.join(timeout=5)
        self.process.stdin.close()
        self.process.stdout.close()
        error = self.process.stderr.read().decode(errors="replace")
        self.process.stderr.close()
        return error


def verify(executable, *, mode):
    probe = Probe(executable)
    try:
        assert probe.event("ready")["sandboxed"] is True
        if mode == "stepping":
            probe.send({"probe": "benchmark"})
            probe.event("benchmark")
        probe.command("Debugger.enable")
        probe.command("Debugger.setBreakpointByUrl", {
            "url": "file:///transmog-script/main.js", "lineNumber":3,
            "condition":"(globalThis.getterCalls++,true)",
        }, error=True)
        pending = probe.command("Debugger.setBreakpointByUrl", {
            "url": "file:///transmog-script/main.js", "lineNumber": 3,
        })
        assert pending["locations"] == []
        probe.send({"probe": "load"})
        probe.event("loaded")
        assert any(v.get("method") == "Debugger.breakpointResolved" for v in probe.transcript)
        probe.send({"probe": "invoke"})
        paused = probe.event("Debugger.paused")["params"]
        frame = paused["callFrames"][0]
        assert frame["functionName"] == "inner", frame
        assert frame["location"]["lineNumber"] == 3, frame
        assert any(f["functionName"] == "outer" for f in paused["callFrames"])
        scope = next(s for s in frame["scopeChain"] if s["type"] == "local")
        values = probe.command("Runtime.getProperties", {
            "objectId": scope["object"]["objectId"], "ownProperties": True,
            "generatePreview": False,
        })["result"]
        values = {v["name"]: v for v in values}
        assert values["local"]["value"]["value"] == 7, values
        properties = probe.command("Runtime.getProperties", {
            "objectId": values["object"]["value"]["objectId"],
            "ownProperties": True, "generatePreview": False,
        })["result"]
        accessor = next(v for v in properties if v["name"] == "secret")
        assert "get" in accessor and "value" not in accessor, accessor
        probe.command("Runtime.evaluate", {"expression": "1+1"}, error=True)
        if mode == "cancellation":
            probe.send({"probe": "stop"})
        elif mode == "disconnect":
            probe.process.stdin.close()
        elif mode == "malformed":
            probe.process.stdin.write(struct.pack("<I", FRAME_BYTES + 1))
            probe.process.stdin.flush()
        else:
            probe.command("Debugger.stepInto" if mode == "step-into" else "Debugger.stepOver")
            stepped = probe.event("Debugger.paused")["params"]["callFrames"][0]
            assert stepped["location"]["lineNumber"] == 4, stepped
            probe.command("Debugger.stepOut")
            stepped = probe.event("Debugger.paused")["params"]["callFrames"][0]
            assert stepped["functionName"] == "outer", stepped
            probe.command("Debugger.resume")
            assert probe.event("completed")["value"] == {"Ok": "14:0"}
            probe.send({"probe": "stop"})
        probe.process.wait(timeout=5)
        assert probe.process.returncode == 0, probe.process.returncode
        return probe.transcript
    finally:
        error = probe.close()
        if error:
            print(error)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path,
                        default=ROOT / "artifacts/script-debugging/initial/phase-0")
    args = parser.parse_args()
    if os.name != "nt":
        raise RuntimeError("The AppContainer Inspector proof requires Windows")
    args.artifacts.mkdir(parents=True, exist_ok=True)
    for name in ("stepping", "step-into", "cancellation", "disconnect", "malformed"):
        transcript = verify(args.executable.resolve(strict=True), mode=name)
        (args.artifacts / f"{name}.json").write_text(
            json.dumps(transcript, indent=2) + "\n", encoding="utf-8")
        print(f"Inspector {name}: passed inside AppContainer")


if __name__ == "__main__":
    main_guard(main)
