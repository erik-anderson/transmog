"""Benchmark streaming content throughput, latency, cancellation and resident memory."""
import argparse
import ctypes
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import time
from repo_tools import ROOT, cargo_output, command, development_environment, main_guard, native_name, write_json


def resident_bytes(process):
    if os.name == "nt":
        from ctypes import wintypes
        class Counters(ctypes.Structure):
            _fields_ = [("cb", wintypes.DWORD), ("PageFaultCount", wintypes.DWORD),
                        *[(name, ctypes.c_size_t) for name in ("PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage", "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage", "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage")]]
        counters = Counters()
        counters.cb = ctypes.sizeof(counters)
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.K32GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
        kernel.K32GetProcessMemoryInfo.restype = wintypes.BOOL
        if not kernel.K32GetProcessMemoryInfo(int(process._handle), ctypes.byref(counters), counters.cb):
            raise ctypes.WinError(ctypes.get_last_error())
        return counters.WorkingSetSize
    if platform.system() == "Linux":
        status = Path(f"/proc/{process.pid}/status").read_text()
        for line in status.splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1]) * 1024
        return 0
    result = command(["ps", "-o", "rss=", "-p", str(process.pid)], check=False, capture=True)
    return int(result.stdout.strip() or "0") * 1024


def measure(binary, argument, interval, environment):
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        with subprocess.Popen([str(binary), argument], cwd=ROOT, env=environment, stdout=output, stderr=errors) as process:
            peak = 0
            try:
                while process.poll() is None:
                    try:
                        peak = max(peak, resident_bytes(process))
                    except (ProcessLookupError, FileNotFoundError):
                        pass
                    except OSError:
                        if process.poll() is None:
                            raise
                    time.sleep(interval / 1000)
                if process.returncode:
                    errors.seek(0)
                    raise RuntimeError(f"Benchmark child {argument} failed: {errors.read().decode('utf-8', errors='replace')}")
            except BaseException:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                raise
            output.seek(0)
            return dict(sampledPeakWorkingSetBytes=peak, output=json.load(output))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--peak-working-set-limit-bytes", type=int, default=512 * 1024 * 1024)
    parser.add_argument("--sample-interval-milliseconds", type=int, default=5)
    parser.add_argument("--output-path", type=Path, default=Path("artifacts/performance/content-pipeline.json"))
    args = parser.parse_args()
    if args.peak_working_set_limit_bytes < 1 or not 1 <= args.sample_interval_milliseconds <= 1000:
        parser.error("Memory limit must be positive; sample interval must be 1-1000 ms.")
    env = development_environment()
    command(["cargo", "build", "--locked", "--release", "-p", "transmog-content", "--example", "content_pipeline_benchmark"], env=env)
    binary = cargo_output(env) / "release/examples" / native_name("content_pipeline_benchmark")
    metrics = json.loads(command([binary, "--json"], env=env, capture=True).stdout)
    if len(metrics["throughput"]) != 8 or len(metrics["firstByte"]) != 8:
        raise ValueError("Benchmark did not report all coding/output combinations.")
    if any(m["bytesPerSecond"] <= 0 or m["nanosPerOperation"] <= 0 for m in metrics["throughput"]):
        raise ValueError("Invalid throughput accounting.")
    if metrics["cancellation"]["maxNanos"] > metrics["cancellation"]["deadlineNanos"]:
        raise ValueError("Cancellation responsiveness exceeded its explicit deadline.")
    if any(m["encodedBytesBeforeOutput"] > m["encodedBodyBytes"] for m in metrics["firstByte"]):
        raise ValueError("Invalid first-byte accounting.")
    baseline = measure(binary, "--memory-baseline", args.sample_interval_milliseconds, env)
    workload = measure(binary, "--memory-probe", args.sample_interval_milliseconds, env)
    base_peak, work_peak = baseline["sampledPeakWorkingSetBytes"], workload["sampledPeakWorkingSetBytes"]
    if min(base_peak, work_peak) <= 0:
        raise ValueError("The sampler did not capture positive resident-memory measurements.")
    if work_peak > args.peak_working_set_limit_bytes:
        raise ValueError(f"Sampled peak working set {work_peak} exceeded {args.peak_working_set_limit_bytes} bytes.")
    report = {"schemaVersion": 1, "recordedAtUtc": datetime.now(timezone.utc).isoformat(),
        "commit": command(["git", "rev-parse", "HEAD"], capture=True).stdout.strip(),
        "workingTreeDirty": bool(command(["git", "status", "--porcelain"], capture=True).stdout.strip()),
        "operatingSystem": platform.platform(), "architecture": platform.machine(), "benchmark": metrics,
        "memory": {"sampleIntervalMilliseconds": args.sample_interval_milliseconds,
            "peakWorkingSetLimitBytes": args.peak_working_set_limit_bytes,
            "baselineSampledPeakWorkingSetBytes": base_peak, "workloadSampledPeakWorkingSetBytes": work_peak,
            "incrementalSampledPeakWorkingSetBytes": max(0, work_peak - base_peak), "baseline": baseline["output"], "workload": workload["output"]}}
    destination = ROOT / args.output_path
    write_json(destination, report)
    print(json.dumps(report, indent=2))
    print(f"Saved benchmark report to {destination}")


if __name__ == "__main__":
    main_guard(main)
