"""Build the native SDK and copy it beside the .NET embedding sample."""
import argparse
import os
import shutil
import sys
from pathlib import Path
from repo_tools import ROOT, command, development_environment, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--configuration", choices=("Debug", "Release"), default="Debug")
    parser.add_argument("--sdk-output", type=Path, help="Consume an existing SDK installation without running Crubit")
    args = parser.parse_args()
    if args.sdk_output is not None:
        env = dict(os.environ)
        install = args.sdk_output.resolve()
    else:
        env = development_environment()
        command([sys.executable, ROOT / "scripts/build-sdk.py", "--configuration", args.configuration], env=env)
        compiler = command(["rustc", "-vV"], env=env, capture=True).stdout
        host = next(line.split(": ", 1)[1] for line in compiler.splitlines() if line.startswith("host:"))
        install = ROOT / ".local/sdk" / host / args.configuration / "install"
    project = ROOT / "examples/dotnet-embedding/Transmog.Embedding.Sample/Transmog.Embedding.Sample.csproj"
    command(["dotnet", "build", project, "--configuration", args.configuration], env=env)
    names = {"win32": "transmog_sdk.dll", "linux": "libtransmog_sdk.so"}
    if sys.platform not in names:
        raise RuntimeError(f"Unsupported native platform: {sys.platform}")
    source = install / ("bin" if sys.platform == "win32" else "lib") / names[sys.platform]
    destination = project.parent / "bin" / args.configuration / "net10.0"
    shutil.copy2(source, destination)
    print(f"SAMPLE_OUTPUT={destination}")


if __name__ == "__main__":
    main_guard(main)
