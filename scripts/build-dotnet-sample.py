"""Build the native bridge and copy it beside the .NET embedding sample."""
import argparse
import shutil
import sys
from repo_tools import ROOT, cargo_output, command, development_environment, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--configuration", choices=("Debug", "Release"), default="Debug")
    args = parser.parse_args()
    env = development_environment()
    argv = ["cargo", "build", "--locked", "-p", "transmog-dotnet-sample-native"]
    if args.configuration == "Release":
        argv.append("--release")
    command(argv, env=env)
    project = ROOT / "examples/dotnet-embedding/Transmog.Embedding.Sample/Transmog.Embedding.Sample.csproj"
    command(["dotnet", "build", project, "--configuration", args.configuration], env=env)
    names = {"win32": "transmog_dotnet_sample_native.dll", "linux": "libtransmog_dotnet_sample_native.so", "darwin": "libtransmog_dotnet_sample_native.dylib"}
    if sys.platform not in names:
        raise RuntimeError(f"Unsupported native platform: {sys.platform}")
    source = cargo_output(env) / args.configuration.lower() / names[sys.platform]
    destination = project.parent / "bin" / args.configuration / "net10.0"
    shutil.copy2(source, destination)
    print(f"SAMPLE_OUTPUT={destination}")


if __name__ == "__main__":
    main_guard(main)
