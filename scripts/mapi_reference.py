"""Pinned offline MAPI reference inputs and developer-only native oracle."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / "third_party/mapi-inspector"
WORK = ROOT / ".tools/mapi-build"
BUILD = ROOT / "extensions/mapi/build"


def verify_sources(vendor: Path = VENDOR) -> dict:
    receipt = json.loads((vendor / "source.json").read_text(encoding="utf-8"))
    expected = receipt["files"]
    actual = {path.relative_to(vendor / "Source").as_posix() for path in (vendor / "Source").rglob("*.cs")}
    if actual != expected.keys():
        raise ValueError("The MAPI reference inventory changed")
    for name, digest in expected.items():
        if hashlib.sha256((vendor / "Source" / name).read_bytes()).hexdigest() != digest:
            raise ValueError(f"The pinned MAPI reference changed: {name}")
    return receipt


def replace_once(source: str, old: str, new: str) -> str:
    if source.count(old) != 1:
        raise ValueError("The pinned MAPI adapter no longer matches its source")
    return source.replace(old, new)


def headless_sources() -> dict[str, str]:
    """Remove only Fiddler UI fallback and legacy capture-file JSON export."""
    verify_sources()
    sources = {name: (VENDOR / "Source" / name).read_text(encoding="utf-8-sig")
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
        subprocess.run([dotnet(), "build", str(BUILD / "Oracle.csproj"), "--configuration", "Release", "--nologo", "-p:RestoreLockedMode=true"], cwd=ROOT, check=True)
    return [dotnet(), str(WORK / "Oracle-bin/Release/net10.0/Oracle.dll")]


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inventory", action="store_true")
    parser.add_argument("--verify", action="store_true")
    arguments = parser.parse_args()
    if arguments.verify:
        receipt = verify_sources()
        print(f'Verified {len(receipt["files"])} reference files at {receipt["revision"]}')
    else:
        raise SystemExit(subprocess.run(oracle_command() + (["--inventory"] if arguments.inventory else []), cwd=ROOT).returncode)
