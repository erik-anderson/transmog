"""Qualify Stored ZIP + TMCap versus Deflate using synthetic local data only.

Build `cargo build --release -p transmog-portable --example package_codec` first.
This measures codec composition separately from application staging/semantic checks.
"""
import argparse
import ctypes
import gzip
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile


def process_metrics(process):
    if sys.platform != 'win32':
        return 0, 0.0
    from ctypes import wintypes
    class Counters(ctypes.Structure):
        _fields_ = [('cb', wintypes.DWORD), ('faults', wintypes.DWORD)] + [
            (name, ctypes.c_size_t) for name in ['peak', 'working', 'paged_peak', 'paged', 'nonpaged_peak', 'nonpaged', 'pagefile', 'pagefile_peak']]
    value = Counters()
    value.cb = ctypes.sizeof(value)
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.K32GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    kernel.K32GetProcessMemoryInfo(int(process._handle), ctypes.byref(value), value.cb)
    created, exited, system, user = (wintypes.FILETIME() for _ in range(4))
    kernel.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
    kernel.GetProcessTimes(int(process._handle), ctypes.byref(created), ctypes.byref(exited), ctypes.byref(system), ctypes.byref(user))
    ticks = lambda item: (item.dwHighDateTime << 32) | item.dwLowDateTime
    return value.peak, (ticks(system) + ticks(user)) / 10_000_000


def sample(command):
    start = time.perf_counter()
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8')
    peak, cpu = 0, 0.0
    while process.poll() is None:
        memory, seconds = process_metrics(process)
        peak, cpu = max(peak, memory), max(cpu, seconds)
        time.sleep(0.02)
    memory, seconds = process_metrics(process)
    peak, cpu = max(peak, memory), max(cpu, seconds)
    output, error = process.communicate()
    if process.returncode:
        raise RuntimeError(error[-4000:])
    return output, {'elapsedSeconds': time.perf_counter() - start, 'cpuSeconds': cpu, 'peakWorkingSetBytes': peak}


def make_zip(recipe, output, compression):
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_STORED if compression == 'stored' else zipfile.ZIP_DEFLATED,
                         compresslevel=None if compression == 'stored' else int(compression)) as archive:
        for path in recipe:
            with Path(path).open('rb') as source, archive.open(Path(path).name, 'w', force_zip64=True) as target:
                shutil.copyfileobj(source, target, 128 * 1024)


def main():
    if len(sys.argv) > 1 and sys.argv[1] == '_zip':
        make_zip(json.loads(Path(sys.argv[2]).read_text(encoding='utf-8')), sys.argv[3], sys.argv[4])
        return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=Path('target/release/examples/package_codec.exe'))
    parser.add_argument('--output', type=Path, default=Path('artifacts/case-transfer-performance.json'))
    args = parser.parse_args()
    binary = str(args.binary.resolve())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    records = []
    with tempfile.TemporaryDirectory(prefix='portable-benchmark-', dir=args.output.parent) as temporary:
        root = Path(temporary)
        capture, encrypted = root / 'capture.tmcap', root / 'encrypted.tmcap'
        sample([binary, 'generate', str(capture)])
        sample([binary, 'generate', str(encrypted), 'encrypted'])
        text, context, asset = root / 'response.txt', root / 'context.json', root / 'response.gz'
        with text.open('wb') as output:
            for _ in range(256):
                output.write((b'const feature = "large plain response";\n' * 2048)[:65536])
        context.write_text(json.dumps({'purpose': 'synthetic config', 'stdout': 'setting=value\r\n' * 8192}), encoding='utf-8')
        with capture.open('rb') as source:
            asset.write_bytes(gzip.compress(source.read(8 * 1024 * 1024), compresslevel=1, mtime=0))
        workloads = {'native-capture': [capture], 'encrypted-capture': [encrypted], 'encoded-asset': [asset],
                     'plain-text': [text], 'small-context': [context],
                     'mixed-native-first': [encrypted, capture, asset, text, context],
                     'mixed-text-first': [context, text, asset, capture, encrypted]}
        for name, paths in workloads.items():
            recipe = root / 'recipe.json'
            recipe.write_text(json.dumps([str(p) for p in paths]), encoding='utf-8')
            for compression in ['stored', '1', '6']:
                archive = root / 'input.zip'
                _, zip_metrics = sample([sys.executable, str(Path(__file__).resolve()), '_zip', str(recipe), str(archive), compression])
                policies = ['whole', 'boundary'] if compression == 'stored' else ['raw']
                for policy in policies:
                    output, metrics = sample([binary, str(archive), str(root / 'wrapped.tmcap'), policy])
                    record = json.loads(output)
                    record.update(workload=name, zipCompression=compression, zipCreation=zip_metrics, codecProcess=metrics)
                    records.append(record)
                if name == 'mixed-native-first' and compression == 'stored':
                    output, metrics = sample([binary, str(archive), str(root / 'protected.tmcap'), 'boundary', 'encrypted'])
                    record = json.loads(output)
                    record.update(workload=name, zipCompression=compression, zipCreation=zip_metrics, codecProcess=metrics)
                    records.append(record)
            print(f'Qualified composition workload: {name}', flush=True)
    args.output.write_text(json.dumps({'scope': 'Synthetic codec composition; excludes app preparation and semantic validation. Warm OS cache; no throughput promise.',
                                      'platform': sys.platform, 'memoryMetric': 'Sampled peak Windows working set per process; KDF initialization reported separately. Zero means unavailable.',
                                      'records': records}, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
