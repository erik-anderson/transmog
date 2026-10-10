"""Regression checks for process ownership before the startup handshake."""
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location('load_process_host', Path(__file__).with_name('process-host.py'))
process_host = importlib.util.module_from_spec(spec)
spec.loader.exec_module(process_host)


class StartupCleanupTests(unittest.TestCase):
    def check_startup_failure(self, emitter_failure):
        owned = []
        real_popen = subprocess.Popen

        def launch(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            if args[0][0] == sys.executable:
                owned.append(child)
            return child

        workspace = Path(__file__).resolve().parents[2]
        test_root = workspace / 'artifacts' / 'load-harness-tests'
        test_root.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=test_root) as directory:
            directory = Path(directory).resolve()
            self.assertTrue(directory.is_relative_to(test_root.resolve()))
            config = directory / 'process.json'
            config.write_text(json.dumps(dict(binary=sys.executable,
                arguments=['-c', 'import time; time.sleep(60)'], environment={},
                log=str(directory / 'target.log'), closeHelper='unused')), encoding='utf-8')
            try:
                with patch.object(process_host.subprocess, 'Popen', side_effect=launch), \
                     patch.object(sys, 'argv', ['process-host.py', '--kind', 'cli', '--config', str(config)]):
                    if emitter_failure:
                        read_fd, write_fd = os.pipe()
                        os.close(read_fd)
                        # Use an actual pipe with its reader closed, including
                        # the real emit()/print()/flush() startup handshake.
                        with io.TextIOWrapper(io.FileIO(write_fd, 'w'), write_through=True) as output:
                            with patch.object(sys, 'stdout', output), self.assertRaises(OSError):
                                process_host.main()
                    else:
                        with patch.object(process_host, 'emit'), patch.object(process_host.threading, 'Thread') as thread:
                            thread.return_value.ident = None
                            thread.return_value.start.side_effect = RuntimeError('sampler startup failed')
                            with self.assertRaisesRegex(RuntimeError, 'sampler startup failed'):
                                process_host.main()
                            thread.return_value.join.assert_not_called()
                self.assertEqual(len(owned), 1)
                self.assertIsNotNone(owned[0].poll(), 'startup failure leaked the owned target')
            finally:
                # Also clean up when running this regression against the old,
                # leaking implementation. Every process here was created above.
                for child in owned:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=5)

    def test_closed_startup_pipe_reaps_target(self):
        self.check_startup_failure(True)

    def test_sampler_start_failure_reaps_target(self):
        self.check_startup_failure(False)


if __name__ == '__main__':
    unittest.main()
