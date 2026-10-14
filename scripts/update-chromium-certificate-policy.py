"""Refresh or check public certificate policy source data from an exact Chromium release."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import urllib.request
from datetime import datetime

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / 'third_party/chromium-certificate-policy'
SOURCES = {
    'cert_verify_proc_blocklist.inc': 'net/cert/cert_verify_proc_blocklist.inc',
    'log_list.json': 'components/certificate_transparency/data/log_list.json',
    'certificate_transparency.proto': 'components/certificate_transparency/certificate_transparency.proto',
    'certificate_transparency_config.proto': 'components/certificate_transparency/certificate_transparency_config.proto',
    'ct_timestamp.proto': 'components/certificate_transparency/ct_timestamp.proto',
    'LICENSE': 'LICENSE',
}

def bundled_ct_config(value):
    """Generate the CTConfig wire format from Chromium's public JSON baseline."""
    def varint(n):
        out = bytearray()
        while n > 127:
            out.append((n & 127) | 128)
            n >>= 7
        out.append(n)
        return bytes(out)
    def number(tag, n):
        return varint(tag << 3) + varint(n)
    def field(tag, data):
        if isinstance(data, str):
            data = data.encode()
        return varint((tag << 3) | 2) + varint(len(data)) + data
    def timestamp(text):
        instant = datetime.fromisoformat(text.replace('Z', '+00:00'))
        return number(1, int(instant.timestamp())) + number(2, instant.microsecond * 1000)
    data = json.loads(value)
    major, minor = map(int, data['version'].split('.'))
    logs = number(1, major) + number(2, minor) + field(3, timestamp(data['log_list_timestamp'])) + number(4, 4)
    states = {'pending': 1, 'qualified': 2, 'usable': 3, 'readonly': 4, 'retired': 5, 'rejected': 6}
    for operator in data['operators']:
        for kind, log_type in [('logs', 1), ('tiled_logs', 2)]:
            for log in operator.get(kind, []):
                state, detail = next(iter(log['state'].items()))
                # Match make_ct_known_logs_list.py's compiled baseline filter.
                if state in ('pending', 'rejected'):
                    continue
                item = field(1, log['description']) + field(2, log['key']) + field(3, log['log_id'])
                item += field(8, number(1, states[state]) + field(2, timestamp(detail['timestamp'])))
                history = sorted(log.get('previous_operators', []),
                                 key=lambda x: datetime.fromisoformat(x['end_time'].replace('Z', '+00:00')),
                                 reverse=True)
                current_start = history[0]['end_time'] if history else '1970-01-01T00:00:00Z'
                item += field(9, field(1, operator['name']) + field(2, timestamp(current_start)))
                for i, previous in enumerate(history):
                    start = history[i + 1]['end_time'] if i + 1 < len(history) else '1970-01-01T00:00:00Z'
                    item += field(9, field(1, previous['name']) + field(2, timestamp(start)))
                item += number(18, log_type)
                logs += field(6, item)
    return field(2, logs)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument('--chromium-version')
    selection.add_argument('--check', action='store_true')
    args = parser.parse_args()
    if args.check:
        receipt = json.loads((DEST / 'snapshot.json').read_text(encoding='utf-8'))
        for name, digest in receipt['sha256'].items():
            if name not in SOURCES or hashlib.sha256((DEST/name).read_bytes()).hexdigest() != digest:
                raise SystemExit(f'Certificate policy source mismatch: {name}')
        if set(receipt['sha256']) != set(SOURCES):
            raise SystemExit('Certificate policy receipt omits source data')
        if (DEST / 'ct_config.pb').read_bytes() != bundled_ct_config((DEST / 'log_list.json').read_bytes()):
            raise SystemExit('Bundled CTConfig does not match its public JSON source')
        print('Verified Chromium ' + receipt['chromiumVersion'] + ' certificate policy source hashes')
        return
    if not re.fullmatch(r'\d+\.\d+\.\d+\.\d+', args.chromium_version):
        parser.error('Use an exact four-part Chromium release')
    base = 'https://raw.githubusercontent.com/chromium/chromium/' + args.chromium_version + '/'
    data = {}
    for name, path in SOURCES.items():
        with urllib.request.urlopen(base+path, timeout=30) as response:
            value = response.read(4*1024*1024+1)
        if len(value) > 4*1024*1024 or not value:
            raise ValueError('Invalid certificate policy source size: '+name)
        data[name] = value
    if b'kSPKIBlockList' not in data['cert_verify_proc_blocklist.inc']:
        raise ValueError('Missing Chromium key blocklist')
    logs = json.loads(data['log_list.json'])
    if not logs.get('operators') or not logs.get('log_list_timestamp'):
        raise ValueError('Invalid public CT source')
    generated = bundled_ct_config(data['log_list.json'])
    DEST.mkdir(exist_ok=True)
    for name, value in data.items():
        (DEST/name).write_bytes(value)
    (DEST / 'ct_config.pb').write_bytes(generated)
    receipt = {'chromiumVersion': args.chromium_version, 'source': base,
               'sha256': {name:hashlib.sha256(value).hexdigest() for name, value in data.items()}}
    (DEST/'snapshot.json').write_text(json.dumps(receipt,indent=2)+'\n',encoding='utf-8',newline='\n')
    print('Updated Chromium certificate policy source data. Review and qualify before committing.')

if __name__ == '__main__':
    main()
