"""Qualify in-use CLI replacement, rollback, shutdown hints, and later-start cleanup on Windows."""
import argparse
import ctypes
from ctypes import wintypes
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import tempfile
import threading
import time
from mcp_test_support import stop_connector


BINARIES = ("transmog.exe", "transmog-cli.exe", "transmog-script-host.exe", "transmog-preview-worker.exe")
HIDDEN = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
ROOT = Path(__file__).resolve().parent.parent


class ProbeTemporaryDirectory(tempfile.TemporaryDirectory):
    """Allow WebView2 to release profile files after the desktop host exits."""
    def __init__(self, *args, cleanup_timeout=30, **kwargs):
        super().__init__(*args, **kwargs)
        self.cleanup_timeout = cleanup_timeout

    def cleanup(self):
        deadline = time.monotonic() + self.cleanup_timeout
        while True:
            try:
                super().cleanup()
                return
            except OSError as error:
                remaining = deadline - time.monotonic()
                # Only retry Windows sharing violations, never hide a persistent
                # lock or other cleanup error. No unrelated browser is stopped.
                if getattr(error, "winerror", None) != 32 or remaining <= 0:
                    raise
                time.sleep(min(0.1, remaining))


def hashes(directory):
    result = {}
    for name in BINARIES:
        with (directory / name).open("rb") as file:
            result[name] = hashlib.file_digest(file, "sha256").hexdigest()
    return result


def initialize(path, environment):
    process = subprocess.Popen([str(path), "mcp", "serve"], env=environment, stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8", creationflags=HIDDEN)
    try:
        process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=1, method="initialize", params=dict(
            protocolVersion="2025-11-25", capabilities={}, clientInfo=dict(name="update-qualification", version="1")))) + "\n")
        process.stdin.flush()
        messages = queue.Queue()
        reader = threading.Thread(target=lambda: messages.put(process.stdout.readline()), daemon=True)
        reader.start()
        assert "result" in json.loads(messages.get(timeout=10)), "Offline initialization failed"
        reader.join(timeout=1)
        process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
        process.stdin.flush()
        return process
    except BaseException:
        stop_connector(process)
        raise


def fixture_installer(payload, install, directory, makensis, utils):
    template = (ROOT / "apps/desktop/windows/installer.nsi").read_text(encoding="utf-8")
    section = template[template.index("Section Install\n"):template.index("  ; Create file associations", template.index("Section Install\n"))] + "\nSectionEnd\n"
    # Render the actual production extraction/apply path, without registration,
    # shortcuts or host-state changes. The signed-release mode runs the full setup.
    section = re.sub(r"\{\{#each binaries\}\}(.*?)\{\{/each\}\}", lambda match: "".join(
        match[1].replace("{{this}}", name).replace("{{no-escape @key}}", str(payload / name)) for name in BINARIES[1:]), section, flags=re.S)
    section = re.sub(r"\{\{#each resources(?:_dirs)?\}\}.*?\{\{/each\}\}", "", section, flags=re.S)
    assert "{{" not in section, "Installer template rendering changed"
    source = r'''
Unicode true
RequestExecutionLevel user
SilentInstall silent
SetCompress off
!include MUI2.nsh
!include FileFunc.nsh
!include x64.nsh
!include Win\COM.nsh
!include Win\Propkey.nsh
!include Win\RestartManager.nsh
!addplugindir "@PLUGINS@"
!include "@UTILS@"
!define MAINBINARYNAME "transmog"
!define MAINBINARYSRCPATH "@MAIN@"
!define PRODUCTNAME "Owned Transmog update qualification"
Var PassiveMode
Name "Owned Transmog update qualification"
OutFile "@OUTPUT@"
InstallDir "@INSTALL@"
LangString appRunning 1033 "Owned desktop is running"
LangString appRunningOkKill 1033 "Close owned desktop"
LangString failedToKillApp 1033 "Could not close owned desktop"
@SECTION@
'''
    installer = directory / "update.exe"
    for token, value in {"@PLUGINS@": makensis.parent / "Plugins/x86-unicode/additional", "@UTILS@": utils,
        "@MAIN@": payload / "transmog.exe", "@OUTPUT@": installer, "@INSTALL@": install, "@SECTION@": section}.items():
        source = source.replace(token, str(value))
    path = directory / "update.nsi"
    path.write_text(source, encoding="utf-8")
    result = subprocess.run([str(makensis), "/V2", str(path)], capture_output=True, text=True, creationflags=HIDDEN)
    (directory / "compile.txt").write_text(result.stdout + result.stderr, encoding="utf-8")
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    return installer


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binaries", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--installer", type=Path)
    parser.add_argument("--installed-directory", type=Path)
    parser.add_argument("--makensis", type=Path, default=Path(os.environ.get("LOCALAPPDATA", "")) / "tauri/NSIS/makensis.exe")
    parser.add_argument("--utils", type=Path, default=ROOT / "target/debug/nsis/x64/utils.nsh")
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("Windows executable mapping and NSIS qualification require Windows")
    if bool(args.installer) != bool(args.installed_directory):
        parser.error("--installer and --installed-directory must be used together")
    shell = shutil.which("pwsh") or shutil.which("powershell")
    existing = subprocess.run([shell, "-NoProfile", "-Command", "@(Get-Process -Name transmog -ErrorAction SilentlyContinue).Count"],
        capture_output=True, text=True, check=True, creationflags=HIDDEN)
    if existing.stdout.strip() != "0":
        raise RuntimeError("Close the existing desktop before isolated update qualification")
    payload = args.binaries.resolve(strict=True)
    expected = hashes(payload)
    evidence = args.artifacts.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    # Only this newly created directory is eligible for recursive fixture cleanup.
    with ProbeTemporaryDirectory(prefix="cli-update-", dir=evidence) as temporary:
        directory = Path(temporary).resolve()
        assert directory.is_relative_to(evidence) and directory != evidence
        install = args.installed_directory.resolve(strict=True) if args.installed_directory else directory / "installed"
        if not args.installer:
            install.mkdir()
            for name in BINARIES:
                shutil.copy2(payload / name, install / name)
        installer = args.installer.resolve(strict=True) if args.installer else fixture_installer(payload, install, directory, args.makensis.resolve(strict=True), args.utils.resolve(strict=True))
        environment = dict(os.environ, LOCALAPPDATA=str(directory / "profile"))
        environment.pop("TRANSMOG_ACCESS_KEY", None)
        environment.pop("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", None)
        connector = None
        desktop = None
        report = dict(expectedHashes=expected, signedInstaller=bool(args.installer), desktopStartupCleanup=False)

        def update():
            process = subprocess.Popen([str(installer), "/S", f"/D={install}"], env=environment,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=HIDDEN)
            try:
                return process.wait(timeout=120)
            except BaseException:
                process.kill()
                process.wait(timeout=5)
                raise

        try:
            connector = initialize(install / "transmog-cli.exe", environment)
            assert update() == 0, "Replacement failed with an initialized offline connector"
            assert hashes(install) == expected, "Installer reported success without replacing every binary"
            assert connector.wait(timeout=5) == 0, "Updated connector ignored the installation shutdown hint"
            report["inUseReplacement"] = True
            stop_connector(connector)
            connector = initialize(install / "transmog-cli.exe", environment)
            assert update() == 0
            assert connector.wait(timeout=5) == 0, "Offline connector did not exit after a same-version replacement"
            report["offlineShutdownHint"] = True
            stop_connector(connector)
            connector = None
            # Deny DELETE sharing to exercise real rename failure and rollback.
            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
            kernel.CreateFileW.restype = wintypes.HANDLE
            kernel.CloseHandle.argtypes = [wintypes.HANDLE]
            handle = kernel.CreateFileW(str(install / "transmog-cli.exe"), 0x80000000, 1, None, 3, 0, None)
            assert handle != ctypes.c_void_p(-1).value, ctypes.WinError(ctypes.get_last_error())
            before = hashes(install)
            generation = (install / ".transmog-installation.json").read_bytes()
            try:
                assert update() != 0, "Installer silently ignored a failed rename"
                assert hashes(install) == before, "Failed replacement did not restore the previous binaries"
                assert (install / ".transmog-installation.json").read_bytes() == generation
            finally:
                kernel.CloseHandle(handle)
            assert update() == 0, "Installer could not recover and retry"
            report["renameFailureRollbackAndRetry"] = True
            # Model an interrupted install after a client mapped its replacement.
            # Recovery must retire that running replacement before restoring it.
            connector = initialize(install / "transmog-cli.exe", environment)
            recovery = install / ".transmog-retired/transaction-rollback1234"
            (recovery / "old").mkdir(parents=True)
            (recovery / "new").mkdir()
            shutil.copy2(install / "transmog-cli.exe", recovery / "old/transmog-cli.exe")
            recovery_entries = [dict(name=name, previous=list(bytes.fromhex(expected[name])), replacement=list(bytes.fromhex(expected[name]))) for name in BINARIES]
            (recovery / "transaction.json").write_text(json.dumps(dict(state="prepared", entries=recovery_entries)), encoding="utf-8")
            desktop = subprocess.Popen([str(install / "transmog.exe")], env=environment,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=HIDDEN)
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                assert desktop.poll() is None, "Interrupted-update startup did not present recovery guidance"
                title = subprocess.run([shell, "-NoProfile", "-Command", f"(Get-Process -Id {desktop.pid}).MainWindowTitle"],
                    check=True, capture_output=True, text=True, creationflags=HIDDEN).stdout.strip()
                if title == "Transmog update needs attention":
                    break
                time.sleep(.1)
            else:
                raise RuntimeError("The interrupted-update recovery dialog did not appear")
            assert not (directory / "profile/Transmog/diagnostics.jsonl").exists(), "Desktop initialized against an incomplete executable set"
            subprocess.run([shell, "-NoProfile", "-File", str(ROOT / "scripts/close-desktop-probe-window.ps1"),
                "-ProbeProcessId", str(desktop.pid), "-WindowTitle", "Transmog update needs attention"], check=True, capture_output=True, creationflags=HIDDEN)
            assert desktop.wait(timeout=10) == 0
            desktop = None
            report["interruptedStartupShowsRepair"] = True
            assert update() == 0, "Recovery could not move a mapped replacement aside"
            assert hashes(install) == expected
            assert connector.wait(timeout=5) == 0
            stop_connector(connector)
            connector = None
            report["inUseRollbackRecovery"] = True
            # A retirement directory junction must never redirect cleanup or
            # replacement into other files, even inside this owned test profile.
            linked_install = directory / "linked-install"
            linked_install.mkdir()
            outside = directory / "unrelated"
            outside.mkdir()
            sentinel = outside / "user.tmcap"
            sentinel.write_text("preserve", encoding="utf-8")
            junction = linked_install / ".transmog-retired"
            subprocess.run([os.environ.get("COMSPEC", "cmd.exe"), "/d", "/c", "mklink", "/J", str(junction), str(outside)],
                check=True, capture_output=True, creationflags=HIDDEN)
            try:
                rejected = subprocess.run([str(payload / "transmog.exe"), "--apply-update-payload", str(linked_install), str(payload)],
                    env=environment, timeout=20, capture_output=True, creationflags=HIDDEN)
                assert rejected.returncode != 0
                assert sentinel.read_text(encoding="utf-8") == "preserve"
                assert list(outside.iterdir()) == [sentinel]
                report["retirementJunctionRejected"] = True
            finally:
                # Remove the junction itself, using native Python throughout.
                assert junction.parent.resolve() == linked_install.resolve()
                os.rmdir(junction)
            # Map the current binary at an owned retired path. This isolates
            # Windows deletion semantics without maintaining legacy binaries.
            transaction = install / ".transmog-retired/transaction-cleanup12345"
            (transaction / "old").mkdir(parents=True)
            (transaction / "new").mkdir()
            retired = transaction / "old/transmog-cli.exe"
            shutil.copy2(payload / "transmog-cli.exe", retired)
            entries = [dict(name=name, previous=list(bytes.fromhex(expected[name])) if name == "transmog-cli.exe" else None,
                            replacement=list(bytes.fromhex(expected[name]))) for name in BINARIES]
            (transaction / "transaction.json").write_text(json.dumps(dict(state="committed", entries=entries)), encoding="utf-8")
            connector = initialize(retired, environment)
            desktop = subprocess.Popen([str(install / "transmog.exe"), "--mcp-start"], env=environment,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=HIDDEN)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                assert desktop.poll() is None, "Updated desktop exited during startup"
                diagnostic = directory / "profile/Transmog/diagnostics.jsonl"
                if diagnostic.exists() and "Transmog application initialized" in diagnostic.read_text(encoding="utf-8") and "retired-cleanup-deferred" in diagnostic.read_text(encoding="utf-8"):
                    report["lockedRetiredFileDeferred"] = retired.exists()
                    break
                time.sleep(.1)
            assert report.get("lockedRetiredFileDeferred"), "Startup blocked or removed an in-use retired executable"
            shell = shutil.which("pwsh") or shutil.which("powershell")
            subprocess.run([shell, "-NoProfile", "-File", str(ROOT / "scripts/close-desktop-probe-window.ps1"),
                "-ProbeProcessId", str(desktop.pid), "-WindowTitle", "Transmog"], check=True, capture_output=True, creationflags=HIDDEN)
            assert desktop.wait(timeout=20) == 0
            desktop = None
            stop_connector(connector)
            connector = None
            desktop = subprocess.Popen([str(install / "transmog.exe"), "--mcp-start"], env=environment,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=HIDDEN)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                assert desktop.poll() is None, "Desktop exited during cleanup retry"
                if not retired.exists():
                    report["desktopStartupCleanup"] = True
                    break
                time.sleep(.1)
            assert report["desktopStartupCleanup"], "A later app start did not delete the released retired executable"
            subprocess.run([shell, "-NoProfile", "-File", str(ROOT / "scripts/close-desktop-probe-window.ps1"),
                "-ProbeProcessId", str(desktop.pid), "-WindowTitle", "Transmog"], check=True, capture_output=True, creationflags=HIDDEN)
            assert desktop.wait(timeout=20) == 0
            desktop = None
        finally:
            if connector is not None:
                stop_connector(connector)
            if desktop is not None:
                desktop.kill()
                desktop.wait(timeout=5)
    report["temporaryDirectoryCleanup"] = True
    (evidence / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
