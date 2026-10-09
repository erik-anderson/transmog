"""Windows CLI lifecycle smoke test using owned profiles and loopback HTTP only.

No certificate is installed and host proxy settings are never configured.
Each CLI gets a hidden new console; Ctrl+C targets only that console.
"""
import argparse
import ctypes
import gzip
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import uuid


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

    def capture(name, persistent=False, crash=False):
        output = fixture / (name + ('.tmcap' if crash else '.tmcap.gz'))
        log_path = fixture / (name + '.log')
        startup = subprocess.STARTUPINFO()
        startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = subprocess.SW_HIDE
        options = ['record', '--output', str(output), '--no-install-root', '--no-system-proxy']
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
                time.sleep(.5)  # Allow the async recorder to persist its valid prefix.
                process.kill()
                process.wait(timeout=10)
                assert list(ledger.glob('root-*.json')) and not list(ledger.glob('*.key'))
            else:
                ctrl_c(process)
                assert process.returncode == 0, log_path.read_text(encoding='utf-8')
                text = log_path.read_text(encoding='utf-8')
                assert 'Stopping capture: restoring proxy settings' in text
                assert 'Trace saved:' in text
                native = fixture / (name + '-decoded.tmcap')
                native.write_bytes(gzip.decompress(output.read_bytes()))
                evidence = fixture / (name + '.jsonl')
                command('capture', 'export', '--input', str(native), '--format', 'jsonl', '--output', str(evidence))
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

    try:
        capture('ephemeral')
        assert not list(ledger.glob('root-*.json')) and not list(ledger.glob('*.key'))
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
        assert 'SEALED=true' in command('capture', 'inspect', '--input', str(recovered))
        assert not list(ledger.glob('root-*.json')) and not list(ledger.glob('*.key'))
        print(json.dumps({'ephemeralMemoryOnly': True, 'persistentReuse': True,
                          'consoleCtrlC': True, 'crashRecovery': True, 'artifacts': str(fixture)}))
    finally:
        for process in processes:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)
        command('roots', 'cleanup', '--include-persistent')
        origin.shutdown()


if __name__ == '__main__':
    main()
