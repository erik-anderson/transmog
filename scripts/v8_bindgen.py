"""Read the native adapter's ABI settings for V8 Rust layout generation."""
import argparse
import json
import os
from pathlib import Path


def bindgen_arguments(project, source_root, environment):
    target = project["targets"]["//:rusty_v8"]
    args = [f"-D{define}" for define in target["defines"]]
    for directory in target["include_dirs"]:
        if directory.startswith("//"):
            directory = str(source_root / directory[2:])
        args.append("-I" + directory)
    for flag in target["cflags"] + target["cflags_cc"]:
        if flag.startswith(("-std=", "-fmsc-version=")) or flag == "-fcomplete-member-pointers":
            args.append(flag)
        elif flag.startswith("/std:c++"):
            args.append("-std=" + flag[5:].removesuffix("preview"))
        elif flag == "/GR-":
            args.append("-fno-rtti")
        elif flag in ("/MD", "/MDd", "/MT", "/MTd"):
            args.append("-D_MT")
            if flag.startswith("/MD"):
                args.append("-D_DLL")
            if flag.endswith("d"):
                args.append("-D_DEBUG")
    if "_HAS_EXCEPTIONS=0" in target["defines"]:
        args.append("-fno-exceptions")
    # The portable environment selects the same MSVC/SDK installation as GN.
    for directory in environment.get("INCLUDE", "").split(";"):
        if directory:
            args.append("-isystem" + directory)
    return args


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gn-out", type=Path, required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    args = parser.parse_args()
    project = json.loads((args.gn_out / "project.json").read_text(encoding="utf-8"))
    print("\0".join(bindgen_arguments(project, args.source_root, os.environ)), end="")


if __name__ == "__main__":
    main()
