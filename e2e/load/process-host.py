"""Own one target process, sample its tree, and stop it through its real exit path.

The JSON-lines control channel never carries traffic or keys. On Windows the CLI
gets a hidden, dedicated console so Ctrl+C cannot reach the user's console.
Only Python's standard library is needed.
"""
import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time


def emit(**value):
    print(json.dumps(value), flush=True)


def windows_memory(root):
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    psapi = ctypes.WinDLL('psapi', use_last_error=True)

    class Entry(ctypes.Structure):
        _fields_ = [('size', wintypes.DWORD), ('usage', wintypes.DWORD),
                    ('pid', wintypes.DWORD), ('heap', ctypes.c_size_t),
                    ('module', wintypes.DWORD), ('threads', wintypes.DWORD),
                    ('parent', wintypes.DWORD), ('priority', wintypes.LONG),
                    ('flags', wintypes.DWORD), ('exe', wintypes.WCHAR * 260)]

    class Counters(ctypes.Structure):
        _fields_ = [('size', wintypes.DWORD), ('faults', wintypes.DWORD)] + [
            (name, ctypes.c_size_t) for name in
            ('peak_rss', 'rss', 'peak_paged', 'paged', 'peak_nonpaged',
             'nonpaged', 'pagefile', 'peak_pagefile', 'private')]

    kernel.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel.OpenProcess.restype = wintypes.HANDLE
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(Entry)]
    kernel.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(Entry)]
    psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    snapshot = kernel.CreateToolhelp32Snapshot(2, 0)
    if snapshot == ctypes.c_void_p(-1).value:
        raise ctypes.WinError(ctypes.get_last_error())
    parents = {}
    try:
        entry = Entry()
        entry.size = ctypes.sizeof(entry)
        ok = kernel.Process32FirstW(snapshot, ctypes.byref(entry))
        while ok:
            parents[entry.pid] = entry.parent
            ok = kernel.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel.CloseHandle(snapshot)
    owned = {root}
    while True:
        expanded = owned | {pid for pid, parent in parents.items() if parent in owned}
        if expanded == owned:
            break
        owned = expanded
    values = []
    for pid in owned:
        handle = kernel.OpenProcess(0x1000 | 0x10, False, pid)
        if not handle:
            continue  # A child may exit between enumeration and sampling.
        try:
            counters = Counters()
            counters.size = ctypes.sizeof(counters)
            if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.size):
                values.append(dict(pid=pid, rss=counters.rss, private=counters.private))
        finally:
            kernel.CloseHandle(handle)
    if not any(value['pid'] == root for value in values):
        raise RuntimeError('Could not sample the owned target process')
    return values


def linux_memory(root):
    parents = {}
    for entry in Path('/proc').iterdir():
        if entry.name.isdigit():
            try:
                parents[int(entry.name)] = int((entry / 'stat').read_text().rsplit(')', 1)[1].split()[1])
            except (OSError, ValueError):
                pass
    owned = {root}
    while True:
        expanded = owned | {pid for pid, parent in parents.items() if parent in owned}
        if expanded == owned:
            break
        owned = expanded
    values = []
    for pid in owned:
        try:
            pages = int(Path(f'/proc/{pid}/statm').read_text().split()[1])
            values.append(dict(pid=pid, rss=pages * os.sysconf('SC_PAGE_SIZE'), private=None))
        except OSError:
            pass
    return values


def stop_cli(child):
    if os.name != 'nt':
        child.send_signal(signal.SIGINT)
        child.wait(timeout=60)
        return
    kernel = ctypes.WinDLL('kernel32', use_last_error=True)
    kernel.FreeConsole()
    if not kernel.AttachConsole(child.pid):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        if not kernel.SetConsoleCtrlHandler(None, True):
            raise ctypes.WinError(ctypes.get_last_error())
        if not kernel.GenerateConsoleCtrlEvent(0, 0):
            raise ctypes.WinError(ctypes.get_last_error())
        child.wait(timeout=60)
    finally:
        kernel.SetConsoleCtrlHandler(None, False)
        kernel.FreeConsole()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--kind', choices=['cli', 'desktop'], required=True)
    parser.add_argument('--config', type=Path, required=True)
    args = parser.parse_args()
    config = json.loads(args.config.read_text(encoding='utf-8'))
    environment = dict(os.environ, **config['environment'])
    startup = None
    flags = 0
    if os.name == 'nt':
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = subprocess.SW_HIDE
        if args.kind == 'cli':
            flags = subprocess.CREATE_NEW_CONSOLE
    done = None
    sampler = None
    with open(config['log'], 'xb') as log:
        child = subprocess.Popen([config['binary'], *config['arguments']],
                                 env=environment, stdin=subprocess.DEVNULL,
                                 stdout=log, stderr=subprocess.STDOUT,
                                 startupinfo=startup, creationflags=flags)
        try:
            # Ownership begins at Popen, before reporting startup or creating
            # the sampler. A closed runner pipe or thread startup failure must
            # still terminate and reap the target.
            done = threading.Event()

            def sample():
                while not done.is_set() and child.poll() is None:
                    try:
                        values = windows_memory(child.pid) if os.name == 'nt' else linux_memory(child.pid)
                        emit(type='sample', timestamp=time.time(), processes=values)
                    except Exception as error:
                        emit(type='sample-error', message=str(error))
                    done.wait(1)
                if not done.is_set():
                    emit(type='target-exited', code=child.returncode)

            emit(type='started', pid=child.pid)
            sampler = threading.Thread(target=sample, daemon=True)
            sampler.start()
            # EOF also cleans up if the runner fails or is interrupted.
            sys.stdin.readline()
            if child.poll() is None:
                if args.kind == 'cli':
                    stop_cli(child)
                else:
                    subprocess.run(['pwsh.exe', '-NoProfile', '-NonInteractive', '-File',
                                    config['closeHelper'], '-ProbeProcessId', str(child.pid),
                                    '-WindowTitle', 'Transmog'], check=True,
                                   stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
                    child.wait(timeout=60)
            emit(type='exited', code=child.returncode)
            if child.returncode:
                raise RuntimeError(f'Target exited with code {child.returncode}')
        finally:
            if done is not None:
                done.set()
            if sampler is not None and sampler.ident is not None:
                sampler.join(timeout=2)
            if child.poll() is None:
                if os.name == 'nt':
                    subprocess.run(['taskkill', '/PID', str(child.pid), '/T', '/F'],
                                   stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
                else:
                    child.kill()
                child.wait(timeout=10)


if __name__ == '__main__':
    main()
