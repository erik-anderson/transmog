"""Pinned on-demand MAPI reference inputs and developer-only native oracle."""
from __future__ import annotations

import hashlib
import json
import os
import re
import tempfile
from urllib.request import urlopen
from zipfile import ZipFile
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / "third_party/mapi-inspector"
WORK = ROOT / ".tools/mapi-build"
BUILD = ROOT / "extensions/mapi/test/reference"
RECEIPT = json.loads((VENDOR / "source.json").read_text(encoding="utf-8"))
SOURCE = WORK / "reference" / RECEIPT["revision"] / "Source"


def verify_sources(source: Path | None = None) -> dict:
    source = SOURCE if source is None else source
    receipt = RECEIPT
    expected = receipt["files"]
    actual = {path.relative_to(source).as_posix() for path in source.rglob("*.cs")}
    if actual != expected.keys():
        raise ValueError("The MAPI reference inventory changed")
    for name, digest in expected.items():
        if hashlib.sha256((source / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"The pinned MAPI reference changed: {name}")
    return receipt


def ensure_sources() -> Path:
    """Fetch only for explicit reference tooling; ordinary builds use checked-in TS."""
    if SOURCE.exists():
        verify_sources()
        return SOURCE
    revision = RECEIPT["revision"]
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("The MAPI reference revision must be immutable")
    SOURCE.parent.mkdir(parents=True, exist_ok=True)
    url = f"https://codeload.github.com/OfficeDev/Office-Inspectors-for-Fiddler/zip/{revision}"
    with tempfile.TemporaryDirectory(dir=SOURCE.parent, prefix="fetch-") as temporary:
        stage = Path(temporary)
        archive = stage / "reference.zip"
        with urlopen(url, timeout=30) as response, archive.open("wb") as output:
            total = 0
            while chunk := response.read(1024 * 1024):
                total += len(chunk)
                if total > 128 * 1024 * 1024:
                    raise ValueError("The reference archive exceeds its download limit")
                output.write(chunk)
        source = stage / "Source"
        with ZipFile(archive) as bundle:
            prefix = f"Office-Inspectors-for-Fiddler-{revision}/MAPIInspector/Source/"
            for name, digest in RECEIPT["files"].items():
                if name.startswith("/") or "\\" in name or any(part in ["", ".", ".."] for part in name.split("/")):
                    raise ValueError("Invalid pinned reference member")
                member = bundle.getinfo(prefix + name)
                if member.file_size > 2 * 1024 * 1024:
                    raise ValueError("The reference member exceeds its size limit")
                data = bundle.read(member)
                if hashlib.sha256(data).hexdigest() != digest:
                    raise ValueError(f"The pinned MAPI reference changed: {name}")
                target = source / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
        verify_sources(source)
        try:
            os.replace(source, SOURCE)
        except OSError:
            if not SOURCE.exists():
                raise
            verify_sources()  # Another reference invocation may have published first.
    return SOURCE


def replace_once(source: str, old: str, new: str) -> str:
    if source.count(old) != 1:
        raise ValueError("The pinned MAPI adapter no longer matches its source")
    return source.replace(old, new)


def headless_sources() -> dict[str, str]:
    """Remove only Fiddler UI fallback and legacy capture-file JSON export."""
    source = ensure_sources()
    sources = {name: (source / name).read_text(encoding="utf-8-sig")
               for name in ["MAPIParser.cs", "SessionNavigator.cs", "Utilities.cs"]}
    parser = sources["MAPIParser.cs"]
    marker = "        private class MAPIFrame"
    if parser.count(marker) != 1:
        raise ValueError("The pinned MAPI export boundary changed")
    sources["MAPIParser.cs"] = replace_once(parser[:parser.index(marker)], "using Newtonsoft.Json;", "") + "\n    }\n}\n"
    sources["SessionNavigator.cs"] = replace_once(sources["SessionNavigator.cs"], "FiddlerApplication.UI.GetAllSessions()", "Array.Empty<Session>()")
    utilities = replace_once(sources["Utilities.cs"], "using Newtonsoft.Json;", "")
    sources["Utilities.cs"] = replace_once(utilities, "return JsonConvert.SerializeObject((object)sealTheObject);", 'throw new NotSupportedException("Legacy JSON export is unavailable in the headless parser");')
    return sources


def prepare_oracle() -> None:
    target = WORK / "oracle"
    target.mkdir(parents=True, exist_ok=True)
    for name, source in headless_sources().items():
        (target / name).write_text(source, encoding="utf-8", newline="\n")


def dotnet() -> str:
    result = shutil.which("dotnet")
    if not result:
        raise RuntimeError("Regenerating MAPI reference results requires the .NET 10 SDK")
    return result


def oracle_command(*, build: bool = True) -> list[str]:
    prepare_oracle()
    if build:
        subprocess.run([dotnet(), "build", str(BUILD / "Oracle.csproj"), "--configuration", "Release", "--nologo", "-p:RestoreLockedMode=true", "-p:MapiReferenceSource=" + str(SOURCE)], cwd=ROOT, check=True)
    return [dotnet(), str(WORK / "Oracle-bin/Release/net10.0/Oracle.dll")]


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", action="store_true")
    parser.add_argument("--verify", action="store_true")
    parser.add_argument("--fetch", action="store_true", help="Fetch and verify the pinned reference into the ignored cache")
    arguments = parser.parse_args()
    if arguments.fetch:
        print(ensure_sources())
    elif arguments.verify:
        receipt = verify_sources()
        print(f'Verified {len(receipt["files"])} reference files at {receipt["revision"]}')
    else:
        raise SystemExit(subprocess.run(oracle_command() + (["--inventory"] if arguments.inventory else []), cwd=ROOT).returncode)
