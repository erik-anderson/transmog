"""Shared, standard-library-only helpers for repository development commands."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

if sys.version_info < (3, 11):
    raise SystemExit("Transmog development requires Python 3.11 or newer.")

ROOT = Path(__file__).resolve().parent.parent


def executable(name: str, environment: dict[str, str] | None = None) -> str:
    env = os.environ if environment is None else environment
    found = shutil.which(name, path=env.get("PATH"))
    if found:
        return found
    if name == "docker" and os.name == "nt":
        candidates = [
            Path(env.get("LOCALAPPDATA", "")) / "Programs/DockerDesktop/resources/bin/docker.exe",
            Path(env.get("LOCALAPPDATA", "")) / "Programs/Docker/Docker/resources/bin/docker.exe",
            Path(env.get("ProgramFiles", "C:/Program Files")) / "Docker/Docker/resources/bin/docker.exe",
        ]
        for candidate in candidates:
            if candidate.is_file():
                return str(candidate)
    raise RuntimeError(f"Required command {name!r} was not found on PATH.")


def command(arguments, *, cwd=ROOT, env=None, capture=False, check=True, input=None):
    """Run argv without a shell; propagate failures immediately."""
    argv = [str(value) for value in arguments]
    tool = executable(argv[0], env)
    # Invoke npm through Node instead of feeding arguments to cmd.exe.
    if Path(tool).suffix.lower() in {".cmd", ".bat"}:
        if argv[0] not in {"npm", "npx"}:
            raise RuntimeError(f"Use a native executable instead of a shell launcher: {tool}")
        cli = Path(tool).parent / "node_modules/npm/bin" / f"{argv[0]}-cli.js"
        if not cli.is_file():
            raise RuntimeError(f"Node package-manager entry point is missing: {cli}")
        argv = [executable("node", env), str(cli), *argv[1:]]
    else:
        argv[0] = tool
    if Path(cwd).resolve() == ROOT and Path(tool).name.lower() in {"cargo", "cargo.exe"}:
        from build_cache import automatic, is_build_command
        if is_build_command(argv[1:]):
            automatic(os.environ if env is None else env, arguments=argv[1:])
    return subprocess.run(argv, cwd=cwd, env=env, check=check, text=True,
                          encoding="utf-8", errors="replace", capture_output=capture, input=input)


def native_name(name: str) -> str:
    return name + (".exe" if os.name == "nt" else "")


def cargo_output(environment=None) -> Path:
    env = os.environ if environment is None else environment
    output = Path(env.get("CARGO_TARGET_DIR", "target"))
    return (ROOT / output).resolve()


def version_key(path: Path):
    return tuple(int(part) if part.isdecimal() else part for part in path.name.split("."))


def development_environment(*, root=ROOT, environment=None, platform=None) -> dict[str, str]:
    """Configure native tools in a child environment, without changing the host."""
    env = dict(os.environ if environment is None else environment)
    windows = (os.name == "nt") if platform is None else platform == "win32"
    bins = []
    if windows:
        bins = [root / ".tools/llvm/bin", root / ".tools/cmake/cmake-4.4.4-windows-x86_64/bin",
                root / ".tools/ninja", root / ".tools/nasm/nasm-3.02"]
        bins = [directory for directory in bins if directory.is_dir()]
        vswhere = Path(env.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
        vc = None
        sdk = Path(env.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Windows Kits/10"
        if vswhere.is_file():
            installation = subprocess.run(
                [str(vswhere), "-latest", "-products", "*", "-requires",
                 "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"],
                env=env, capture_output=True, text=True, check=True).stdout.strip()
            if installation:
                tools = Path(installation) / "VC/Tools/MSVC"
                vc = max((p for p in tools.iterdir() if p.is_dir()), key=version_key, default=None)
        if vc and sdk.is_dir():
            versions = [p for p in (sdk / "Include").iterdir() if p.is_dir() and p.name.startswith("10.")]
            preferred = [p for p in versions if p.name.startswith("10.0.26100.")]
            if not versions:
                raise RuntimeError("Windows SDK headers are missing.")
            version = max(preferred or versions, key=version_key).name
            bins += [vc / "bin/Hostx64/x64", sdk / f"bin/{version}/x64"]
            env["LIB"] = ";".join(map(str, [vc / "lib/x64", sdk / f"Lib/{version}/ucrt/x64", sdk / f"Lib/{version}/um/x64"]))
            env["INCLUDE"] = ";".join(map(str, [vc / "include", *[sdk / f"Include/{version}/{part}" for part in ("ucrt", "shared", "um", "winrt")]]))
        elif not vc:
            scope = Path("C:/Program Files/Microsoft Visual Studio/18/Community/SDK/ScopeCppSDK/vc15")
            if scope.is_dir():
                bins += [scope / "VC/bin", scope / "SDK/bin"]
                env["LIB"] = ";".join(map(str, [scope / "VC/lib", scope / "SDK/lib"]))
                env["INCLUDE"] = ";".join(map(str, [scope / "VC/include", *[scope / f"SDK/include/{part}" for part in ("ucrt", "shared", "um")]]))
        env.update(CC="clang-cl", CXX="clang-cl", AR="llvm-lib")
    else:
        env.setdefault("CC", "clang")
        env.setdefault("CXX", "clang++")
        env.setdefault("AR", "llvm-ar")
    separator = ";" if windows else os.pathsep
    env["PATH"] = separator.join([*map(str, bins), env.get("PATH", "")])
    required = ["rustc", "cargo", "clang", "cmake", "ninja", "nasm"]
    required += ["clang-cl", "llvm-lib", "link"] if windows else [env["CC"], env["CXX"], env["AR"]]
    for tool in required:
        executable(tool, env)
    env["CMAKE_GENERATOR"] = "Ninja"
    env["CMAKE_MAKE_PROGRAM"] = executable("ninja", env)
    if windows and not env.get("LIBCLANG_PATH"):
        clang_dir = Path(executable("clang", env)).parent
        if (clang_dir / "libclang.dll").is_file():
            env["LIBCLANG_PATH"] = str(clang_dir)
    env["TRANSMOG_PYTHON"] = sys.executable
    return env


def write_json(path: Path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")


def main_guard(main):
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
