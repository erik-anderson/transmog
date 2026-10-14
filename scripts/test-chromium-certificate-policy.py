"""Offline checks for the reproducible public CT baseline and maintenance receipt."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest

spec = importlib.util.spec_from_file_location('policy', Path(__file__).with_name('update-chromium-certificate-policy.py'))
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)

class SnapshotTests(unittest.TestCase):
    def test_reproducible_read_only_check(self):
        before = {p.name: p.read_bytes() for p in policy.DEST.iterdir() if p.is_file()}
        subprocess.run([sys.executable, policy.__file__, '--check'], check=True)
        after = {p.name: p.read_bytes() for p in policy.DEST.iterdir() if p.is_file()}
        self.assertEqual(before, after)
        self.assertEqual(policy.bundled_ct_config(before['log_list.json']), before['ct_config.pb'])

    def test_moving_reference_rejected_before_network(self):
        result = subprocess.run([sys.executable, policy.__file__, '--chromium-version', 'main'], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_operator_history_orders_fractional_timestamps(self):
        data = json.loads((policy.DEST / 'log_list.json').read_bytes())
        log = data['operators'][0]['logs'][0]
        log['previous_operators'] = [
            {'name': 'Earlier fixture operator', 'end_time': '2026-01-01T00:00:01Z'},
            {'name': 'Later fixture operator', 'end_time': '2026-01-01T00:00:01.900Z'},
        ]
        generated = policy.bundled_ct_config(json.dumps(data).encode())
        self.assertLess(generated.index(b'Later fixture operator'), generated.index(b'Earlier fixture operator'))

if __name__ == '__main__':
    unittest.main()
