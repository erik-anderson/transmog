"""Failure-path subprocess cleanup must remain bounded and reap owned children."""
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mcp_test_support import OwnedProcess, stop_connector


class McpCleanupTests(unittest.TestCase):
    @unittest.skipUnless(OwnedProcess.supported(), "Stable process handles unavailable")
    def test_owned_handle_waits_for_exit_and_survives_process_reaping(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        owned = None
        try:
            owned = OwnedProcess(child.pid)
            self.assertFalse(owned.wait(0))
            owned.kill()
            self.assertTrue(owned.wait(5))
            child.wait(timeout=5)
            # It is still the same terminated process after its PID is released.
            self.assertTrue(owned.wait(0))
            owned.kill()
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)
            if owned is not None:
                owned.close()

    def test_broken_input_and_unresponsive_child_do_not_skip_cleanup(self):
        child = subprocess.Popen([
            sys.executable, "-c",
            "import os,time; os.close(0); print('ready', flush=True); time.sleep(60)"
        ], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        try:
            self.assertEqual(child.stdout.readline().strip(), b"ready")
            child.stdin.write(b"buffered input")
            started = time.monotonic()
            stop_connector(child, timeout=0.1)
            self.assertIsNotNone(child.returncode)
            self.assertLess(time.monotonic() - started, 5)
            self.assertTrue(child.stdin.closed)
            self.assertTrue(child.stdout.closed)
            self.assertTrue(child.stderr.closed)
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)
            for pipe in (child.stdin, child.stdout, child.stderr):
                try:
                    pipe.close()
                except OSError:
                    pass


if __name__ == "__main__":
    unittest.main()
