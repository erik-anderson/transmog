"""Exercise delayed Windows profile locks with real child-process file handles."""
from contextlib import contextmanager
import importlib.util
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
from mcp_test_support import stop_connector

spec = importlib.util.spec_from_file_location("cli_update", SCRIPTS / "test-cli-update.py")
cli_update = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cli_update)

LOCKER = r'''
import ctypes, sys
from ctypes import wintypes
kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
    ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
kernel.CreateFileW.restype = wintypes.HANDLE
kernel.CloseHandle.argtypes = [wintypes.HANDLE]
handle = kernel.CreateFileW(sys.argv[1], 0x80000000, 3, None, 3, 0, None)
if handle == ctypes.c_void_p(-1).value:
    raise ctypes.WinError(ctypes.get_last_error())
try:
    print("locked", flush=True)
    sys.stdin.buffer.read(1)
finally:
    kernel.CloseHandle(handle)
'''


@contextmanager
def locked_profile(directory):
    path = Path(directory) / "profile/Transmog/WebView2/EBWebView/Default/Favicons"
    path.parent.mkdir(parents=True)
    path.write_text("owned fixture", encoding="utf-8")
    process = subprocess.Popen([sys.executable, "-c", LOCKER, str(path)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, encoding="utf-8", creationflags=subprocess.CREATE_NO_WINDOW)
    try:
        ready = queue.Queue()
        reader = threading.Thread(target=lambda: ready.put(process.stdout.readline()), daemon=True)
        reader.start()
        if ready.get(timeout=5).strip() != "locked":
            raise RuntimeError("Profile-lock fixture failed to acquire its file handle")
        reader.join(timeout=1)
        yield process
    finally:
        stop_connector(process)


def release_later(process):
    release = threading.Timer(0.3, process.stdin.close)
    release.daemon = True
    release.start()
    return release


@unittest.skipUnless(os.name == "nt", "Windows delete-sharing semantics")
class ProfileCleanupTests(unittest.TestCase):
    def test_delayed_browser_lock_reproduces_failure_then_cleans_only_owned_directory(self):
        with tempfile.TemporaryDirectory(prefix="cleanup-regression-") as parent:
            sentinel = Path(parent) / "unrelated.tmcap"
            sentinel.write_text("preserve", encoding="utf-8")
            probe = cli_update.ProbeTemporaryDirectory(dir=parent, cleanup_timeout=5)
            with probe as directory:
                # The probe owns this newly created child of the test root.
                self.assertEqual(Path(directory).resolve().parent, Path(parent).resolve())
                with locked_profile(directory) as child:
                    # This is the exact old TemporaryDirectory cleanup path.
                    with self.assertRaises(PermissionError) as original:
                        tempfile.TemporaryDirectory.cleanup(probe)
                    self.assertEqual(original.exception.winerror, 32)
                    release = release_later(child)
                    try:
                        probe.cleanup()
                        self.assertFalse(Path(directory).exists())
                    finally:
                        release.join(timeout=5)
            self.assertEqual(sentinel.read_text(encoding="utf-8"), "preserve")

    def test_persistent_lock_remains_a_bounded_failure(self):
        with tempfile.TemporaryDirectory(prefix="cleanup-regression-") as parent:
            probe = cli_update.ProbeTemporaryDirectory(dir=parent, cleanup_timeout=0.1)
            try:
                with locked_profile(probe.name):
                    started = time.monotonic()
                    with self.assertRaises(PermissionError) as failure:
                        probe.cleanup()
                    self.assertEqual(failure.exception.winerror, 32)
                    self.assertLess(time.monotonic() - started, 3)
                    self.assertTrue(Path(probe.name).is_dir())
            finally:
                probe.cleanup()

    def test_waiting_for_profile_release_preserves_the_original_probe_failure(self):
        with tempfile.TemporaryDirectory(prefix="cleanup-regression-") as parent:
            probe = cli_update.ProbeTemporaryDirectory(dir=parent, cleanup_timeout=5)
            try:
                with locked_profile(probe.name) as child:
                    release = release_later(child)
                    try:
                        with self.assertRaisesRegex(AssertionError, "original probe failure"):
                            with probe:
                                raise AssertionError("original probe failure")
                        self.assertFalse(Path(probe.name).exists())
                    finally:
                        release.join(timeout=5)
            finally:
                probe.cleanup()


class FailureEvidenceTests(unittest.TestCase):
    def test_failure_evidence_survives_profile_cleanup_without_copying_credentials(self):
        with tempfile.TemporaryDirectory(prefix="failure-evidence-") as parent:
            evidence = Path(parent) / "evidence"
            evidence.mkdir()
            with cli_update.ProbeTemporaryDirectory(dir=parent) as directory:
                profile = Path(directory) / "profile/Transmog"
                profile.mkdir(parents=True)
                diagnostic = '{"code":"startup","message":"owned probe"}\n'
                (profile / "diagnostics.jsonl").write_text(diagnostic, encoding="utf-8")
                (profile / "mcp-access.json").write_text("private-fixture-key", encoding="utf-8")
                cli_update.preserve_failure(Path(directory), evidence, RuntimeError("owned shutdown failed"),
                    {"desktopStartupCleanup": False})
            self.assertFalse(Path(directory).exists())
            self.assertEqual((evidence / "failed-desktop-diagnostics.jsonl").read_text(encoding="utf-8"), diagnostic)
            report = json.loads((evidence / "failure.json").read_text(encoding="utf-8"))
            self.assertEqual(report["status"], "failed")
            self.assertEqual(report["error"], "owned shutdown failed")
            self.assertIs(report["checks"]["desktopStartupCleanup"], False)
            self.assertNotIn("private-fixture-key", "".join(path.read_text(encoding="utf-8") for path in evidence.iterdir()))


    def test_failed_native_command_retains_utf8_byte_output(self):
        with tempfile.TemporaryDirectory(prefix="failure-evidence-") as parent:
            evidence = Path(parent) / "evidence"
            evidence.mkdir()
            error = subprocess.CalledProcessError(1, ["owned-probe"],
                output=b"owned stdout", stderr="native error: \u03a9".encode("utf-8"))
            cli_update.preserve_failure(Path(parent) / "profile", evidence, error, {})
            report = json.loads((evidence / "failure.json").read_text(encoding="utf-8"))
            self.assertEqual(report["stdout"], "owned stdout")
            self.assertEqual(report["stderr"], "native error: \u03a9")
            self.assertEqual(report["status"], "failed")


if __name__ == "__main__":
    unittest.main()
