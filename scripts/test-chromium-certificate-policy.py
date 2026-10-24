"""Offline checks for the reproducible public CT baseline and maintenance receipt."""
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import unittest

spec = importlib.util.spec_from_file_location('policy', Path(__file__).with_name('update-chromium-certificate-policy.py'))
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)

class SnapshotTests(unittest.TestCase):
    def test_compromised_emudhra_intermediate_keys_remain_blocked(self):
        # Chromium added these eight SPKI hashes on 2026-10-20 in net commit
        # de7d6b99731d2f20e253950c761e4dbd85b451e3 after the CA compromise.
        expected = {
            '43bffa19d6ef651451f6841e019931e96f6f2eaeff28ab4b6f1ce4576de2717b',
            '49dd353dc2b919a3770645117d0e60c16115e7d1bc259eefe734b00ea434fe5f',
            '5c0660a2b164910d8ff589c18e14552e9e0ec237dcefe2cb736d41a4811f81a0',
            '63ecefee431d49523bc1136c4e72d0206a477086caf606cd5cf025686c109cf8',
            '92c063d67e8f14db71d288e11266ffdaf69cbf115f7bab14f3bc2f68d25e0e30',
            'e0254c7fcdc265e2f0f8a428dabb75ab4ae039b4872d62f5ee57f0877d5a8a41',
            'ebed6475c2c2d80fcb199f3fe8dd6e6b3ce60b77eb15d4e42c3dec0c6f5b5105',
            'ec4ffa1ad0ac88081314b7aaffef6a91f97cd57291bb5ba686c5415f5eb4f6b6',
        }
        source = (policy.DEST / 'cert_verify_proc_blocklist.inc').read_text(encoding='utf-8')
        blocked = set()
        for block in re.findall(r'\{([^{}]+)\}', source):
            octets = re.findall(r'0x([0-9a-fA-F]{2})', block)
            if len(octets) == 32:
                blocked.add(''.join(octets).lower())
        self.assertTrue(expected <= blocked, f'Missing compromised eMudhra keys: {expected - blocked}')

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
