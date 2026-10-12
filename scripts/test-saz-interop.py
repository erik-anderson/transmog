"""Verify SAZ encryption with 7-Zip and inspect interoperability fixtures."""
import argparse
from datetime import datetime
import hashlib
import os
from pathlib import Path
import re
import shutil
import tempfile
from urllib.request import urlopen
import xml.etree.ElementTree as ET
import zipfile
from repo_tools import ROOT, command, development_environment, main_guard


def verify_metadata(directory):
    required = ("ClientConnected", "ClientDoneRequest", "ServerGotRequest", "ServerDoneResponse", "ClientBeginResponse", "ClientDoneResponse")
    dates = (*required, "ClientBeginRequest", "GotRequestHeaders", "ServerConnected", "FiddlerBeginRequest", "ServerBeginResponse", "GotResponseHeaders")
    archives = list(directory.glob("*-plain.saz"))
    if not archives:
        raise ValueError("No unencrypted SAZ fixtures were found.")
    checked = 0
    for path in archives:
        with zipfile.ZipFile(path) as archive:
            members = [name for name in archive.namelist() if re.fullmatch(r"raw/[0-9]+_m\.xml", name)]
            if not members:
                raise ValueError(f"No session metadata in {path.name}.")
            for member in members:
                timers = ET.fromstring(archive.read(member)).findall(".//SessionTimers")
                if not timers:
                    raise ValueError(f"No SessionTimers in {path.name}/{member}.")
                for timer in timers:
                    for name in dates:
                        value = timer.get(name)
                        if value is None:
                            if name in required:
                                raise ValueError(f"{path.name}/{member}: required timer {name} is missing.")
                        else:
                            datetime.fromisoformat(value)
                checked += 1
    print(f"{checked} sessions in {len(archives)} SAZ fixtures verified.")


def seven_zip(explicit):
    if explicit:
        return str(explicit.resolve(strict=True))
    for name in ("7z", "7za", "7zz"):
        tool = shutil.which(name)
        if tool:
            return tool
    if os.name != "nt":
        raise ValueError("Install 7-Zip or pass --seven-zip-path for the independent ZIP reader test.")
    root = ROOT / ".tools/7zip-26.04"
    root.mkdir(parents=True, exist_ok=True)
    assets = {"7zr.exe": "256feca8e274e5da655e2a284fabafd9f554365eb164862089dacd4e8276d282",
              "7z2604-extra.7z": "dc4b11d3399db18b063630137145f5585d8f7ac847bf3639bd1185d7d1f7cee0"}
    for name, expected in assets.items():
        path = root / name
        if not path.is_file():
            with tempfile.TemporaryDirectory(prefix="download-", dir=root) as temporary:
                staged = Path(temporary) / name
                with urlopen(f"https://github.com/ip7z/7zip/releases/download/26.04/{name}", timeout=60) as response, staged.open("wb") as output:
                    shutil.copyfileobj(response, output)
                with staged.open("rb") as source:
                    if hashlib.file_digest(source, "sha256").hexdigest() != expected:
                        raise ValueError("Official 7-Zip asset checksum mismatch.")
                staged.replace(path)
        with path.open("rb") as source:
            if hashlib.file_digest(source, "sha256").hexdigest() != expected:
                raise ValueError("Official 7-Zip asset checksum mismatch.")
    command([root / "7zr.exe", "x", root / "7z2604-extra.7z", "-o" + str(root / "portable"), "-y"])
    return str(root / "portable/x64/7za.exe")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").digest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seven-zip-path", type=Path)
    args = parser.parse_args()
    tool = seven_zip(args.seven_zip_path)
    env = development_environment()
    artifacts = ROOT / "artifacts/saz-interop"
    artifacts.mkdir(parents=True, exist_ok=True)
    # Retain each run's independent-reader evidence for inspection.
    directory = Path(tempfile.mkdtemp(prefix="run-", dir=artifacts))
    env["TRANSMOG_SAZ_INTEROP_DIR"] = str(directory)
    for case in ("encrypted_saz_round_trips_and_produces_interop_fixtures", "classic_metadata_required_dates_preserve_unavailable_and_original_times", "websocket_saz_exports_frames_in_both_profiles_and_encrypted_archives"):
        command(["cargo", "test", "--locked", "-p", "transmog-saz", case], env=env)
    verify_metadata(directory)
    if os.name == "nt":
        # Keep the additional .NET XmlConvert compatibility check on Windows.
        command(["pwsh", "-NoProfile", "-File", ROOT / "scripts/test-saz-metadata.ps1", "-FixtureDirectory", directory], env=env)
    for profile in ("strict", "extended", "websocket-h1-strict", "websocket-h1-extended", "websocket-h2-strict", "websocket-h2-extended"):
        encrypted = directory / (profile + "-encrypted.saz")
        command([tool, "t", encrypted, "-pTransmog-interop-password"], env=env)
        decoded, reference = directory / (profile + "-decoded"), directory / (profile + "-reference")
        command([tool, "x", encrypted, "-o" + str(decoded), "-pTransmog-interop-password", "-y"], env=env)
        command([tool, "x", directory / (profile + "-plain.saz"), "-o" + str(reference), "-y"], env=env)
        expected = {p.relative_to(reference): digest(p) for p in reference.rglob("*") if p.is_file()}
        actual = {p.relative_to(decoded): digest(p) for p in decoded.rglob("*") if p.is_file()}
        if expected != actual:
            raise ValueError(f"Decrypted {profile} ZIP members changed.")
        if command([tool, "t", encrypted, "-pincorrect"], env=env, check=False, capture=True).returncode == 0:
            raise ValueError("Independent ZIP reader accepted an incorrect password.")
        print(f"{profile} SAZ: independent native decryption and exact member hashes verified.")
    raw = directory / "legacy-source/raw"
    raw.mkdir(parents=True)
    payload = bytes(range(256)) * 1024
    (raw / "1_c.txt").write_bytes(f"POST https://example.test/upload HTTP/1.1\r\nContent-Length: {len(payload)}\r\n\r\n".encode("ascii") + payload)
    command([tool, "a", "-tzip", directory / "external-zipcrypto.saz", "raw/1_c.txt", "-mem=ZipCrypto", "-pTransmog-interop-password"], cwd=raw.parent, env=env)
    command(["cargo", "test", "--locked", "-p", "transmog-saz", "external_zipcrypto_imports_exact_binary_without_extraction"], env=env)


if __name__ == "__main__":
    main_guard(main)
