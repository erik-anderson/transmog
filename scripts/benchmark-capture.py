"""Compare adaptive capture compression with fixed memory and constrained CPU affinity."""
from __future__ import annotations
import argparse
import ctypes
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
from repo_tools import ROOT, command, development_environment, cargo_output, main_guard

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--container', action='store_true')
    parser.add_argument('--encrypted', action='store_true')
    parser.add_argument('--kinds', default='text,binary,mixed,small')
    parser.add_argument('--no-build', action='store_true')
    parser.add_argument('--mib', type=int, default=256)
    parser.add_argument('--repeats', type=int, default=3)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    env = development_environment()
    if not args.no_build:
        command(['cargo', 'build', '--release', '-p', 'transmog-capture', '--example', 'capture_compression_benchmark'], env=env)
    binary = cargo_output(env) / 'release/examples' / ('capture_compression_benchmark.exe' if os.name == 'nt' else 'capture_compression_benchmark')
    if os.name == 'nt':
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        kernel.GetProcessAffinityMask.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t), ctypes.POINTER(ctypes.c_size_t)]
        kernel.SetProcessAffinityMask.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        available, system = ctypes.c_size_t(), ctypes.c_size_t()
        if not kernel.GetProcessAffinityMask(kernel.GetCurrentProcess(), ctypes.byref(available), ctypes.byref(system)):
            raise ctypes.WinError(ctypes.get_last_error())
        cpus = [i for i in range(64) if available.value & (1 << i)]
    else:
        cpus = sorted(os.sched_getaffinity(0))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    rows = []
    with args.output.open('w', encoding='utf-8') as output:
        for core_count in [n for n in (1, 2, 4) if n <= len(cpus)]:
            for kind in args.kinds.split(','):
                for repeat in range(args.repeats):
                    # Alternate ordering to reduce warm-up/thermal bias.
                    for workers in ((1, 2, 4) if repeat % 2 == 0 else (4, 2, 1)):
                        process = subprocess.Popen([str(binary), '--wait', '--mib', str(args.mib), '--workers', str(workers), '--kind', kind] + (['--container'] if args.container else []) + (['--encrypted'] if args.encrypted else []), cwd=ROOT, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                        try:
                            if os.name == 'nt':
                                mask = sum(1 << cpu for cpu in cpus[:core_count])
                                if not kernel.SetProcessAffinityMask(int(process._handle), mask):
                                    raise ctypes.WinError(ctypes.get_last_error())
                            else:
                                os.sched_setaffinity(process.pid, cpus[:core_count])
                            stdout, stderr = process.communicate('start\n', timeout=120)
                        except BaseException:
                            process.kill()
                            process.communicate()
                            raise
                        if process.returncode:
                            raise RuntimeError(stderr)
                        row = json.loads(stdout)
                        row.update(cpu_limit=core_count, repeat=repeat)
                        rows.append(row)
                        output.write(json.dumps(row) + '\n')
                        output.flush()
                group = [r for r in rows if r['cpu_limit'] == core_count and r['kind'] == kind]
                medians = {w: round(statistics.median(r['mib_s'] for r in group if r['workers'] == w), 2) for w in (1, 2, 4)}
                print(json.dumps({'cpu_limit': core_count, 'kind': kind, 'median_mib_s': medians}), flush=True)
    print(f'Raw measurements: {args.output}', flush=True)

if __name__ == '__main__':
    main_guard(main)
