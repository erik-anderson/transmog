"""Refresh or check public certificate policy source data from an exact Chromium release."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
DEST = ROOT / 'third_party/chromium-certificate-policy'
SOURCES = {
    'cert_verify_proc_blocklist.inc': 'net/cert/cert_verify_proc_blocklist.inc',
    'log_list.json': 'components/certificate_transparency/data/log_list.json',
    'certificate_transparency.proto': 'components/certificate_transparency/certificate_transparency.proto',
    'LICENSE': 'LICENSE',
}

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
    DEST.mkdir(exist_ok=True)
    for name, value in data.items():
        (DEST/name).write_bytes(value)
    receipt = {'chromiumVersion': args.chromium_version, 'source': base,
               'sha256': {name:hashlib.sha256(value).hexdigest() for name, value in data.items()}}
    (DEST/'snapshot.json').write_text(json.dumps(receipt,indent=2)+'\n',encoding='utf-8',newline='\n')
    print('Updated Chromium certificate policy source data. Review and qualify before committing.')

if __name__ == '__main__':
    main()
