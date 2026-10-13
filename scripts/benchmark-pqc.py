"""Measure full PQC TLS handshake latency, process CPU and resident memory."""
import argparse
import ctypes
from datetime import datetime, timezone
import importlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import time
from repo_tools import ROOT, cargo_output, command, development_environment, main_guard, native_name, write_json

# Reuse the existing Windows/Linux/macOS resident-memory sampler.
resident_bytes = importlib.import_module("benchmark-content").resident_bytes


def cpu_seconds(process):
    if os.name == "nt":
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetProcessTimes.argtypes = [wintypes.HANDLE, *[ctypes.POINTER(wintypes.FILETIME)] * 4]
        kernel.GetProcessTimes.restype = wintypes.BOOL
        times = [wintypes.FILETIME() for _ in range(4)]
        if not kernel.GetProcessTimes(int(process._handle), *(ctypes.byref(value) for value in times)):
            raise ctypes.WinError(ctypes.get_last_error())
        return sum((value.dwHighDateTime << 32) | value.dwLowDateTime for value in times[2:]) / 10_000_000
    if platform.system() == "Linux":
        fields = Path(f"/proc/{process.pid}/stat").read_text().rsplit(")", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    return None


def measure(binary, signature, group, operations, env):
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        with subprocess.Popen([str(binary), signature, group, str(operations)], cwd=ROOT, env=env,
                              stdout=output, stderr=errors,
                              creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0) as process:
            peak, cpu = 0, None
            deadline = time.monotonic() + 120
            try:
                while process.poll() is None:
                    if time.monotonic() > deadline:
                        raise TimeoutError("PQC benchmark exceeded its deadline")
                    try:
                        peak = max(peak, resident_bytes(process))
                        measured_cpu = cpu_seconds(process)
                        if measured_cpu is not None:
                            cpu = max(cpu or 0, measured_cpu)
                    except (ProcessLookupError, FileNotFoundError):
                        pass
                    except OSError:
                        if process.poll() is None:
                            raise
                    time.sleep(0.005)
                if os.name == "nt":
                    cpu = cpu_seconds(process)
                if process.returncode:
                    errors.seek(0)
                    raise RuntimeError(errors.read().decode("utf-8", errors="replace"))
            except BaseException:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                raise
            output.seek(0)
            row = json.load(output)
            row.update(peakWorkingSetBytes=peak, processCpuSeconds=cpu,
                       cpuMeasurement="process lifetime" if os.name == "nt" else "sampled" if cpu is not None else "unavailable")
            if peak <= 0 or row["clientHelloRecordBytes"] <= 0:
                raise ValueError("Missing resident-memory or ClientHello measurement")
            return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--operations", type=int, default=500)
    parser.add_argument("--output", type=Path, default=Path("artifacts/performance/pqc-handshakes.json"))
    args = parser.parse_args()
    if not 10 <= args.operations <= 10_000:
        parser.error("Operations must be 10-10000")
    env = development_environment()
    command(["cargo", "build", "--locked", "--release", "-p", "transmog-tls", "--example", "pqc_benchmark"], env=env)
    binary = cargo_output(env) / "release/examples" / native_name("pqc_benchmark")
    rows = [measure(binary, signature, group, args.operations, env)
            for signature in ("classical", "mldsa44", "mldsa65", "mldsa87", "automatic")
            for group in ("classical", "hybrid")]
    report = dict(recordedAtUtc=datetime.now(timezone.utc).isoformat(),
                  commit=command(["git", "rev-parse", "HEAD"], capture=True).stdout.strip(),
                  workingTreeDirty=bool(command(["git", "status", "--porcelain"], capture=True).stdout.strip()),
                  operatingSystem=platform.platform(), architecture=platform.machine(),
                  sampleIntervalMilliseconds=5, measurements=rows)
    destination = ROOT / args.output
    write_json(destination, report)
    print(json.dumps(rows, indent=2))
    print(f"Saved PQC handshake measurements to {destination}")


if __name__ == "__main__":
    main_guard(main)
