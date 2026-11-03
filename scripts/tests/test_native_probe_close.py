"""Native close probes must wait for readiness and message processing."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
SHELL = shutil.which("pwsh") or shutil.which("powershell")
FIXTURE = r'''
import ctypes, json, pathlib, sys, threading, time
from ctypes import wintypes as w
user = ctypes.WinDLL("user32", use_last_error=True)
kernel = ctypes.WinDLL("kernel32", use_last_error=True)
callback = ctypes.WINFUNCTYPE(w.LPARAM, w.HWND, w.UINT, w.WPARAM, w.LPARAM)
class WindowClass(ctypes.Structure):
    _fields_ = [("style", w.UINT), ("procedure", callback), ("classExtra", ctypes.c_int),
        ("windowExtra", ctypes.c_int), ("instance", w.HINSTANCE), ("icon", w.HICON),
        ("cursor", w.HANDLE), ("background", w.HBRUSH), ("menu", w.LPCWSTR), ("name", w.LPCWSTR)]
user.DefWindowProcW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
user.DefWindowProcW.restype = w.LPARAM
user.CreateWindowExW.argtypes = [w.DWORD, w.LPCWSTR, w.LPCWSTR, w.DWORD,
    ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, w.HWND, w.HMENU, w.HINSTANCE, ctypes.c_void_p]
user.CreateWindowExW.restype = w.HWND
user.RegisterClassW.argtypes = [ctypes.POINTER(WindowClass)]
user.IsWindowVisible.argtypes = [w.HWND]
user.ShowWindow.argtypes = [w.HWND, ctypes.c_int]
user.GetMessageW.argtypes = [ctypes.POINTER(w.MSG), w.HWND, w.UINT, w.UINT]
user.DispatchMessageW.argtypes = [ctypes.POINTER(w.MSG)]
user.DispatchMessageW.restype = w.LPARAM
kernel.GetModuleHandleW.argtypes = [w.LPCWSTR]
kernel.GetModuleHandleW.restype = w.HMODULE
root = pathlib.Path(sys.argv[1])
mode = sys.argv[2]
@callback
def procedure(window, message, wp, lp):
    if message == 0 and mode == "hung":
        time.sleep(30)
        return 0
    if message == 0x0010:
        # Delivery is deliberately observable after a slow native handler.
        time.sleep(0.75)
        (root / "closed.json").write_text(json.dumps({"visible": bool(user.IsWindowVisible(window))}))
        return 0  # Delivery alone must never be mistaken for process exit.
    return user.DefWindowProcW(window, message, wp, lp)
instance = kernel.GetModuleHandleW(None)
window_class = WindowClass(0, procedure, 0, 0, instance, None, None, None, None, "OwnedCloseFixture")
assert user.RegisterClassW(ctypes.byref(window_class))
window = user.CreateWindowExW(0, "OwnedCloseFixture", "Transmog 0.1.18 Canary", 0x00CF0000,
    0, 0, 300, 180, None, None, instance, None)
assert window
if mode != "hidden":
    user.ShowWindow(window, 5)
else:
    def show_later():
        while not (root / "show").exists():
            time.sleep(.01)
        user.ShowWindow(window, 5)
    threading.Thread(target=show_later, daemon=True).start()
print("ready", flush=True)
message = w.MSG()
while user.GetMessageW(ctypes.byref(message), None, 0, 0) > 0:
    user.DispatchMessageW(ctypes.byref(message))
'''


@contextmanager
def native_window(root, mode="normal"):
    child = subprocess.Popen([sys.executable, "-c", FIXTURE, str(root), mode],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        creationflags=subprocess.CREATE_NO_WINDOW)
    try:
        ready = queue.Queue()
        threading.Thread(target=lambda: ready.put(child.stdout.readline()), daemon=True).start()
        if ready.get(timeout=10).strip() != "ready":
            raise RuntimeError("Native fixture failed: " + child.stderr.read())
        yield child
    finally:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)
        child.stdout.close()
        child.stderr.close()


def helper(child, *extra):
    return [SHELL, "-NoProfile", "-File", str(SCRIPTS / "close-desktop-probe-window.ps1"),
            "-ProbeProcessId", str(child.pid), "-WindowTitle", "Transmog", *extra]


@unittest.skipUnless(os.name == "nt" and SHELL, "Windows native message delivery")
class NativeCloseTests(unittest.TestCase):
    def test_close_message_is_processed_before_the_helper_returns(self):
        with tempfile.TemporaryDirectory(prefix="native-close-") as temporary:
            root = Path(temporary)
            with native_window(root) as child:
                result = subprocess.run(helper(child), capture_output=True, text=True, timeout=20)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue((root / "closed.json").is_file(), "Helper returned before Close was processed")
                self.assertIsNone(child.poll(), "The helper must not force the application to exit")

    def test_hidden_startup_window_is_not_closed_before_it_is_shown(self):
        with tempfile.TemporaryDirectory(prefix="native-close-") as temporary:
            root = Path(temporary)
            with native_window(root, "hidden") as child:
                release = threading.Timer(3, lambda: (root / "show").write_text("show"))
                release.start()
                try:
                    result = subprocess.run(helper(child), capture_output=True, text=True, timeout=20)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertTrue(json.loads((root / "closed.json").read_text())["visible"])
                finally:
                    release.join(timeout=5)

    def test_unresponsive_window_fails_without_sending_close_or_killing_it(self):
        with tempfile.TemporaryDirectory(prefix="native-close-") as temporary:
            root = Path(temporary)
            with native_window(root, "hung") as child:
                result = subprocess.run(helper(child, "-ReadySeconds", "1"), capture_output=True, text=True, timeout=20)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("ready", result.stderr.lower())
                self.assertFalse((root / "closed.json").exists())
                self.assertIsNone(child.poll())


if __name__ == "__main__":
    unittest.main()
