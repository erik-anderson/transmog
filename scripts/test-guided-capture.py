"""Guided CLI smoke: owned hidden consoles, isolated state, loopback traffic only."""
import argparse
import ctypes
from ctypes import wintypes
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
import uuid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--executable', required=True, type=Path)
    parser.add_argument('--artifacts', required=True, type=Path)
    args = parser.parse_args()
    if os.name != 'nt':
        raise SystemExit('Windows console qualification requires Windows.')
    root = args.artifacts.resolve() / uuid.uuid4().hex
    root.mkdir(parents=True)
    executable = args.executable.resolve(strict=True)
    env = dict(os.environ, LOCALAPPDATA=str(root / 'state'))
    profile = root / 'support.json'
    profile.write_text(json.dumps(dict(schemaVersion=1, id='loopback-issue', revision=1,
        title='Controlled customer issue', steps=['Open website', 'Mark the issue'],
        relevantHosts=[dict(host='relevant.test')], durationSeconds=6,
        recording=dict(redactSensitiveHeaders=True,
                       requestBodyLimit=dict(kind='limited', bytes=32), encryption='required'))),
        encoding='utf-8')
    password = root / 'password.txt'
    password.write_text('controlled-fixture-password', encoding='utf-8')

    def command(*options, ok=True):
        result = subprocess.run([str(executable), *map(str, options)], env=env,
            stdin=subprocess.DEVNULL, capture_output=True, text=True, encoding='utf-8', timeout=45)
        assert (result.returncode == 0) == ok, (options, result.stdout, result.stderr)
        return result.stdout if ok else result.stdout + result.stderr

    validated = json.loads(command('profile', 'validate', '--input', profile, '--format', 'json'))
    assert validated['valid'] and not (root / 'state').exists()
    invalid = root / 'invalid.json'
    invalid.write_text(profile.read_text(encoding='utf-8').replace('"schemaVersion": 1', '"schemaVersion": 2'), encoding='utf-8')
    command('record', '--profile', invalid, '--output', root / 'bad.tmcap', ok=False)
    command('record', '--profile', profile, '--password-file', password,
            '--output', root / 'missing-parent' / 'bad.tmcap', ok=False)
    assert not (root / 'state').exists(), 'Invalid input mutated CLI setup state'

    class Origin(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(200)
            self.send_header('Content-Length', '7')
            self.end_headers()
            self.wfile.write(b'fixture')

        def log_message(self, *_):
            pass

    origin = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    # Open the newly allocated console explicitly. Parent stdin is redirected by
    # automation, while the recorder must own a real terminal for this test.
    launcher = root / 'launch.py'
    launcher.write_text('''import json,subprocess,sys
from pathlib import Path
config=json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'))
with open('CONIN$', 'r') as console, open(config['log'], 'wb') as log:
 p=subprocess.Popen(config['args'],stdin=console,stdout=log,stderr=subprocess.STDOUT)
 Path(config['pid']).write_text(str(p.pid))
 sys.exit(p.wait())
''', encoding='utf-8')

    def mark(pid, text):
        kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        class Char(ctypes.Union):
            _fields_ = [('UnicodeChar', wintypes.WCHAR), ('AsciiChar', ctypes.c_char)]
        class Key(ctypes.Structure):
            _fields_ = [('down', wintypes.BOOL), ('repeat', wintypes.WORD),
                        ('virtual', wintypes.WORD), ('scan', wintypes.WORD),
                        ('char', Char), ('control', wintypes.DWORD)]
        class Event(ctypes.Union):
            _fields_ = [('key', Key), ('padding', ctypes.c_byte * 16)]
        class Record(ctypes.Structure):
            _fields_ = [('type', wintypes.WORD), ('event', Event)]
        kernel.FreeConsole()
        if not kernel.AttachConsole(pid):
            raise ctypes.WinError(ctypes.get_last_error())
        kernel.CreateFileW.restype = wintypes.HANDLE
        kernel.WriteConsoleInputW.argtypes = [wintypes.HANDLE, ctypes.POINTER(Record), wintypes.DWORD, ctypes.POINTER(wintypes.DWORD)]
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = kernel.CreateFileW('CONIN$', 0xC0000000, 3, None, 3, 0, None)
        try:
            records = (Record * len(text))()
            for record, char in zip(records, text):
                record.type = 1
                record.event.key.down = True
                record.event.key.repeat = 1
                record.event.key.virtual = 13 if char == '\r' else 0
                record.event.key.char.UnicodeChar = char
            written = wintypes.DWORD()
            if not kernel.WriteConsoleInputW(handle, records, len(records), ctypes.byref(written)):
                raise ctypes.WinError(ctypes.get_last_error())
            assert written.value == len(records)
        finally:
            kernel.CloseHandle(handle)
            kernel.FreeConsole()

    def capture(name, circular=False, crash=False):
        output, log, pid_file = (root / (name + suffix) for suffix in ['.tmcap', '.log', '.pid'])
        options = [str(executable), 'record', '--profile', str(profile), '--output', str(output),
            '--password-file', str(password), '--no-install-root', '--no-system-proxy']
        if circular:
            options += ['--circular-buffer', '1MiB']
        config = root / (name + '.json')
        config.write_text(json.dumps(dict(args=options, log=str(log), pid=str(pid_file))), encoding='utf-8')
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = subprocess.SW_HIDE
        process = subprocess.Popen([sys.executable, str(launcher), str(config)], env=env,
            creationflags=subprocess.CREATE_NEW_CONSOLE, startupinfo=startup)
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                text = log.read_text(encoding='utf-8', errors='replace') if log.exists() else ''
                if 'Recording. Reproduce' in text:
                    break
                assert process.poll() is None, text
                time.sleep(.1)
            else:
                raise AssertionError('Recorder did not start: ' + text)
            endpoint = re.search(r'Proxy address: (127\.0\.0\.1:\d+)', text).group(1)
            connection = http.client.HTTPConnection(endpoint)
            connection.request('GET', f'http://127.0.0.1:{origin.server_port}/unrelated?private=fixture',
                               headers={'Cookie': 'private-fixture-cookie'})
            response = connection.getresponse()
            assert response.status == 200 and response.read() == b'fixture'
            connection.close()
            mark(process.pid, 'm Controlled issue\r')
            while time.monotonic() < deadline:
                text = log.read_text(encoding='utf-8', errors='replace')
                if 'Issue marker 1 recorded:' in text:
                    break
                assert process.poll() is None, text
                time.sleep(.1)
            else:
                raise AssertionError('Marker was not acknowledged: ' + text)
            if crash:
                subprocess.run(['taskkill', '/PID', pid_file.read_text(), '/F'], check=True, capture_output=True)
            process.wait(timeout=30)
            text = log.read_text(encoding='utf-8', errors='replace')
            if not crash:
                assert process.returncode == 0, text
                assert 'Requested duration reached' in text and 'File sealed: true' in text
                assert '0 relevant' in text, text
            inspect = command('capture', 'export', '--input', output, '--password-file', password, '--format', 'jsonl')
            assert 'incident-marker' in inspect, inspect
            rows = [json.loads(line) for line in inspect.splitlines() if line.strip()]
            incident = [row['payload'] for row in rows if row.get('kind') == 'incident-marker']
            assert len(incident) == 1 and incident[0]['note'] == 'Controlled issue'
            heads = [row['payload'] for row in rows if row.get('kind') == 'request-head']
            assert any('127.0.0.1' in head['target'] for head in heads), 'Advisory hosts filtered unrelated traffic'
            cookies = [header for head in heads for header in head['headers'] if bytes(header['name']).lower() == b'cookie']
            assert cookies and all(header.get('value') is None for header in cookies), cookies
            assert str(password) not in inspect and 'controlled-fixture-password' not in inspect

            return str(output)
        finally:
            if process.poll() is None:
                if pid_file.exists():
                    subprocess.run(['taskkill', '/PID', pid_file.read_text(), '/F'], capture_output=True)
                process.kill()
                process.wait()

    try:
        outputs = [capture('normal'), capture('circular', circular=True), capture('checkpoint-crash', crash=True)]
        # A normal subsequent capture uses the existing initial preferences.
        plain = command('record', '--duration', '1', '--output', root / 'ordinary.tmcap',
                        '--no-install-root', '--no-system-proxy')
        assert 'complete, including credentials and cookies' in plain, plain
        assert '25000000' in command('capture', 'export', '--input', root / 'ordinary.tmcap', '--format', 'jsonl')
        command('roots', 'cleanup', '--include-persistent')
        result = dict(passed=True, outputs=outputs, profileValidation=True,
                      invalidInputBeforeSetup=True, interactiveMarkers=True,
                      encryptedCircular=True, checkpointRecovery=True, runScopedPreferences=True)
        (root / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps(dict(artifacts=str(root), **result)))
    finally:
        origin.shutdown()


if __name__ == '__main__':
    main()
