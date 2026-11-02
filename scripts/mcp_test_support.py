"""Bounded cleanup for MCP subprocesses owned by a qualification check."""
import os
import select
import signal
import subprocess


class OwnedProcess:
    """Keep a stable process handle so cleanup cannot signal a reused PID."""
    @staticmethod
    def supported():
        return os.name == "nt" or (hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"))

    def __init__(self, pid):
        if os.name == "nt":
            import ctypes
            from ctypes import wintypes
            self.api = ctypes.WinDLL("kernel32", use_last_error=True)
            self.api.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
            self.api.OpenProcess.restype = wintypes.HANDLE
            self.api.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
            self.api.WaitForSingleObject.restype = wintypes.DWORD
            self.api.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
            self.api.TerminateProcess.restype = wintypes.BOOL
            self.api.CloseHandle.argtypes = [wintypes.HANDLE]
            self.api.CloseHandle.restype = wintypes.BOOL
            self.handle = self.api.OpenProcess(0x00100001, False, pid)  # SYNCHRONIZE | TERMINATE
            if not self.handle:
                raise ctypes.WinError(ctypes.get_last_error())
        else:
            self.handle = os.pidfd_open(pid)

    def wait(self, seconds):
        if os.name == "nt":
            import ctypes
            result = self.api.WaitForSingleObject(self.handle, int(seconds * 1000))
            if result not in (0, 258):  # signaled / timed out
                raise ctypes.WinError(ctypes.get_last_error())
            return result == 0
        return bool(select.select([self.handle], [], [], seconds)[0])

    def kill(self):
        if self.wait(0):
            return
        if os.name == "nt":
            import ctypes
            if not self.api.TerminateProcess(self.handle, 1) and not self.wait(0):
                raise ctypes.WinError(ctypes.get_last_error())
        else:
            try:
                signal.pidfd_send_signal(self.handle, signal.SIGKILL)
            except ProcessLookupError:
                pass

    def close(self):
        if os.name == "nt":
            self.api.CloseHandle(self.handle)
        else:
            os.close(self.handle)


def stop_connector(process, timeout=5):
    """Close input, wait, then kill and reap; broken input cannot skip cleanup."""
    try:
        if process.stdin:
            process.stdin.close()
    except OSError:
        pass
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
    finally:
        for pipe in (process.stdout, process.stderr):
            if pipe:
                pipe.close()
