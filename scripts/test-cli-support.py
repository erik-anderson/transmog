"""Windows CLI lifecycle smoke test using owned profiles and loopback HTTP only.

No certificate is installed and host proxy settings are never configured.
Each CLI gets a hidden new console; Ctrl+C targets only that console.
"""
import argparse
import ctypes
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import threading
import time
import uuid
import zlib
import zipfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--executable', required=True, type=Path)
    parser.add_argument('--artifacts', required=True, type=Path)
    args = parser.parse_args()
    if os.name != 'nt':
        raise SystemExit('This test exercises Windows console controls.')
    executable = args.executable.resolve(strict=True)
    fixture = args.artifacts.resolve() / ('cli-' + uuid.uuid4().hex)
    fixture.mkdir(parents=True)
    profile = fixture / 'profile'
    profile.mkdir()
    environment = dict(os.environ, LOCALAPPDATA=str(profile))
    ledger = profile / 'Transmog-cli'
    processes = []
    expected = b'controlled support request\x00\xff'

    class Origin(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            body = self.rfile.read(int(self.headers['Content-Length']))
            assert body == expected
            self.send_response(200)
            self.send_header('Content-Type', 'text/plain')
            self.send_header('Content-Length', '8')
            self.end_headers()
            self.wfile.write(b'captured')

        def log_message(self, *_):
            pass

    origin = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()

    def command(*options):
        return subprocess.run([str(executable), *options], env=environment,
                              stdin=subprocess.DEVNULL, capture_output=True, text=True, check=True).stdout

    def rejected(*options):
        result = subprocess.run([str(executable), *options], env=environment,
                                stdin=subprocess.DEVNULL, capture_output=True, text=True)
        assert result.returncode != 0, result.stdout
        return result.stderr

    def ctrl_c(process):
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        kernel.FreeConsole()
        if not kernel.AttachConsole(process.pid):
            raise ctypes.WinError(ctypes.get_last_error())
        try:
            # Ignore Ctrl+C in this helper; the already running CLI does not
            # inherit this setting. Its console contains no user processes.
            if not kernel.SetConsoleCtrlHandler(None, True):
                raise ctypes.WinError(ctypes.get_last_error())
            if not kernel.GenerateConsoleCtrlEvent(0, 0):
                raise ctypes.WinError(ctypes.get_last_error())
            process.wait(timeout=30)
        finally:
            kernel.SetConsoleCtrlHandler(None, False)
            kernel.FreeConsole()

    def capture(name, persistent=False, crash=False, circular=None, encrypted=False, conflict=False):
        output = fixture / (name + '.tmcap')
        log_path = fixture / (name + '.log')
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = subprocess.SW_HIDE
        options = ['record', '--output', str(output), '--no-install-root', '--no-system-proxy']
        password_file = fixture / 'fixture-password.txt'
        password_file.write_text('controlled-test-password', encoding='utf-8')
        if circular:
            options += ['--circular-buffer', circular]
        if encrypted:
            options += ['--encrypt', '--password-file', str(password_file)]
        if persistent:
            options.append('--persistent-root')
        with log_path.open('wb') as log:
            process = subprocess.Popen([str(executable), *options], env=environment,
                                       stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
                                       startupinfo=startup, creationflags=subprocess.CREATE_NEW_CONSOLE)
            processes.append(process)
            for _ in range(600):
                text = log_path.read_text(encoding='utf-8', errors='replace')
                if 'Recording. Reproduce' in text:
                    break
                if process.poll() is not None:
                    raise AssertionError(text)
                time.sleep(.05)
            else:
                raise AssertionError('CLI did not reach Recording')
            if circular:
                assert not output.exists()
                if circular != 'unlimited':
                    assert not list((ledger / 'circular').glob('*'))
            assert 'press Ctrl+C once' in text
            assert '25000000 bytes per request' in text
            records = [json.loads(path.read_text()) for path in ledger.glob('root-*.json')]
            assert len(records) == 1
            record = records[0]
            assert record['lifecycle']['state'] == 'manual'
            assert record['lifecycle']['expiresAt'] > record['lifecycle']['createdAt']
            assert bool(list(ledger.glob('*.key'))) == persistent
            assert record['lifecycle']['keyStorage'] == ('protected-file' if persistent else 'memory-only')
            assert not any('PRIVATE KEY' in path.read_text() for path in ledger.glob('*.json'))
            endpoint = re.search(r'Proxy address: ([^\r\n]+)', text).group(1)
            host, port = endpoint.rsplit(':', 1)
            connection = http.client.HTTPConnection(host, int(port), timeout=10)
            connection.request('POST', f'http://127.0.0.1:{origin.server_port}/support', expected,
                               {'Authorization': 'Bearer controlled-fixture', 'Content-Type': 'application/octet-stream'})
            response = connection.getresponse()
            assert response.status == 200 and response.read() == b'captured'
            connection.close()
            if crash:
                # Wait for the bounded periodic checkpoint, not an assumed
                # per-event filesystem write. Preserve actual body evidence.
                deadline = time.monotonic() + 5
                while True:
                    result = subprocess.run(
                        [str(executable), 'capture', 'inspect', '--input', str(output)],
                        env=environment, stdin=subprocess.DEVNULL,
                        capture_output=True, text=True)
                    retained = re.search(r'RETAINED_BODY_BYTES=(\d+)', result.stdout)
                    if result.returncode == 0 and retained and int(retained.group(1)) > 0:
                        break
                    if time.monotonic() >= deadline:
                        raise AssertionError('No body checkpoint published within five seconds')
                    time.sleep(.05)
                process.kill()
                process.wait(timeout=10)
                assert list(ledger.glob('root-*.json')) and not list(ledger.glob('*.key'))
            else:
                if conflict:
                    output.write_bytes(b'existing user file')
                ctrl_c(process)
                text = log_path.read_text(encoding='utf-8')
                assert 'Stopping capture: restoring proxy settings' in text
                if conflict:
                    assert process.returncode != 0, text
                    assert output.read_bytes() == b'existing user file'
                    native = Path(re.search(r'preserved at (.+?)\. Use capture inspect', text).group(1))
                    assert native.is_file(), text
                    assert f'The native evidence remains at {output}' not in text
                else:
                    assert process.returncode == 0, text
                    assert 'Trace saved:' in text
                    native = output
                evidence = fixture / (name + '.jsonl')
                password_args = ['--password-file', str(password_file)] if encrypted else []
                command('capture', 'export', '--input', str(native), '--format', 'jsonl', '--output', str(evidence), *password_args)
                if encrypted:
                    rejected = subprocess.run([str(executable), 'capture', 'inspect', '--input', str(native)], env=environment, stdin=subprocess.DEVNULL, capture_output=True, text=True)
                    assert rejected.returncode != 0 and 'password' in rejected.stderr.lower()
                    assert 'SEALED=true' in command('capture', 'inspect', '--input', str(native), '--password-file', str(password_file))
                assert 'certificateContext' in evidence.read_text()
                def retained_authorization(value):
                    if isinstance(value, dict):
                        if value.get('name') in (list(b'Authorization'), list(b'authorization')):
                            return value.get('value') == list(b'Bearer controlled-fixture')
                        return any(retained_authorization(child) for child in value.values())
                    if isinstance(value, list):
                        return any(retained_authorization(child) for child in value)
                    return False
                assert any(retained_authorization(json.loads(line))
                           for line in evidence.read_text().splitlines())
        return record, output

    def export_rejections(source):
        # Valid checksummed native fixtures can have incomplete or unsafe heads.
        data = source.read_bytes()
        header_end = 13 + struct.unpack('<I', data[9:13])[0]
        header = data[:header_end]
        header_json = data[13:header_end]
        geometry = json.loads(header_json)
        assert geometry['cipher'] == 'none' and geometry['layout'] == 'indexed-blocks'
        block_size = geometry['block_size']
        payload_size = block_size - 5
        block_bytes = block_size + 16

        def write_native(path, records):
            # Independent raw-codec encoder keeps this rejection fixture valid
            # without relying on the production writer to accept unsafe heads.
            records = [*records, {'revision': 4, 'sequence': 0, 'exchange_id': 0,
                                  'kind': 'seal', 'payload': {'record_count': len(records)}}]
            encoded = bytearray(header + struct.pack('<I', block_size) + bytes(512))
            start = len(encoded)
            ordinal = 0

            def put(kind, payload):
                nonlocal ordinal
                assert len(payload) <= payload_size
                plain = bytes([kind]) + struct.pack('<I', len(payload)) + payload
                plain += bytes(block_size - len(plain))
                aad = header_json + struct.pack('<Q', ordinal) + b'TMCap block'
                encoded.extend(plain + hashlib.sha256(aad + plain).digest()[:16])
                ordinal += 1

            def packed(value):
                payload = json.dumps(value, separators=(',', ':')).encode()
                return b'\0' + struct.pack('<I', len(payload)) + payload

            logical = bytearray()
            entries = []
            for index, record in enumerate(records):
                payload = json.dumps(record, separators=(',', ':')).encode()
                frame = struct.pack('<QIB', index * 2 + 1, len(payload), 0) + payload
                frame = struct.pack('<II', len(frame), zlib.crc32(frame)) + frame
                offset = start + len(logical) // payload_size * block_bytes + 5 + len(logical) % payload_size
                entries.append({'offset': offset, 'frame_bytes': len(frame),
                                'index': index, 'stored': record})
                logical.extend(frame)
            for offset in range(0, len(logical), payload_size):
                put(0, logical[offset:offset + payload_size])
            first_index = ordinal
            index = packed(entries)
            for offset in range(0, len(index), payload_size):
                put(1, index[offset:offset + payload_size])
            root = {'block': first_index, 'blocks': ordinal - first_index,
                    'bytes': len(index), 'first': 0, 'count': len(entries), 'level': 0}
            put(3, packed({'roots': [root], 'count': len(entries), 'sealed': True, 'block': ordinal}))
            path.write_bytes(encoded)

        request = {'revision': 4, 'sequence': 1, 'exchange_id': 1, 'kind': 'request-head',
                   'payload': {'boundary': 'client-request', 'method': 'GET',
                               'target': 'http://example.invalid/', 'headers': []}}
        source = fixture / 'missing-response.tmcap'
        write_native(source, [request])
        for mode in ('saz', 'saz-extended'):
            output = fixture / (mode + '-placeholder.saz')
            command('capture', 'export', '--input', str(source), '--format', mode,
                    '--output', str(output))
            with zipfile.ZipFile(output) as archive:
                assert archive.read('raw/1_s.txt').startswith(b'HTTP/1.1 599 Transmog response not captured')
                assert b'ui-comments' in archive.read('raw/1_m.xml')

        request['payload']['headers'] = [{'name': list(b'x-fixture'), 'value': list(b'bad\r\nvalue')}]
        response = {'revision': 4, 'sequence': 2, 'exchange_id': 1, 'kind': 'response-head',
                    'payload': {'boundary': 'client-response', 'status': 200, 'headers': []}}
        write_native(source, [request, response])
        output = fixture / 'failed-export.saz'
        for _ in range(2):
            error = rejected('capture', 'export', '--input', str(source), '--format', 'saz',
                             '--output', str(output))
            assert 'unsafe HTTP wire field' in error, error
            assert not output.exists()

    try:
        for topic in ('record', 'serve', 'ca generate', 'ca protect', 'ca issue', 'roots cleanup',
                      'capture inspect', 'capture validate', 'capture seal', 'capture export'):
            assert 'Usage:' in command(*topic.split(), '--help')
        assert 'persist' in command('help', 'record')
        assert '--source-password-file' in command('help', 'capture', 'export')
        bad_output = fixture / 'missing-directory' / 'trace.tmcap'
        error = rejected('record', '--output', str(bad_output), '--circular-buffer', 'auto',
                         '--no-install-root', '--no-system-proxy')
        assert 'Cannot prepare destination' in error and not ledger.exists(), error
        error = rejected('serve', '--ca-cert', 'missing.pem', '--ca-key', 'missing.key',
                         '--proof-id', 'invalid\r\nvalue')
        assert '--proof-id' in error and 'panicked' not in error, error
        _, ordinary = capture('ephemeral')
        export_rejections(ordinary)
        assert not list(ledger.glob('root-*.json')) and not list(ledger.glob('*.key'))
        capture('circular-memory', circular='auto', encrypted=True)
        capture('circular-disk', circular='unlimited', encrypted=True)
        capture('circular-conflict', circular='auto', encrypted=True, conflict=True)
        capture('encrypted-stream', encrypted=True)
        first, _ = capture('persistent-1', persistent=True)
        state = json.loads(next(ledger.glob('root-*.json')).read_text())
        assert state['lifecycle']['state'] == 'persistent-idle'
        assert state['lifecycle']['outcome'] == 'capture-sealed'
        second, _ = capture('persistent-2', persistent=True)
        assert first['sha256'] == second['sha256']
        assert first['lifecycle']['runId'] != second['lifecycle']['runId']
        command('roots', 'cleanup', '--include-persistent')
        _, interrupted = capture('interrupted', crash=True)
        command('roots', 'cleanup')
        recovered = fixture / 'recovered.tmcap'
        command('capture', 'seal', '--input', str(interrupted), '--output', str(recovered))
        recovered_summary = command('capture', 'inspect', '--input', str(recovered))
        assert 'SEALED=true' in recovered_summary
        assert int(re.search(r'RETAINED_BODY_BYTES=(\d+)', recovered_summary).group(1)) > 0
        assert not list(ledger.glob('root-*.json')) and not list(ledger.glob('*.key'))
        print(json.dumps({'ephemeralMemoryOnly': True, 'persistentReuse': True,
                          'consoleCtrlC': True, 'crashRecovery': True, 'encryptedStreaming': True,
                          'circularMemory': True, 'circularDisk': True, 'circularSaveRecovery': True,
                          'commandHelp': True, 'sazPlaceholders': True,
                          'atomicExports': True, 'proofValidation': True, 'artifacts': str(fixture)}))
    finally:
        for process in processes:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
        command('roots', 'cleanup', '--include-persistent')
        origin.shutdown()


if __name__ == '__main__':
    main()
