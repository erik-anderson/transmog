"""Shared, standard-library-only helpers for repository development commands."""
from __future__ import annotations

import json
import hashlib
import ntpath
import os
import platform as host_platform
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
    if Path(tool).name.lower() in {"cargo", "cargo.exe"}:
        from build_cache import automatic, is_build_command
        if is_build_command(argv[1:]):
            env = dict(os.environ if env is None else env)
            env["TRANSMOG_SOURCE_STATE"] = json.dumps({
                "root": str(ROOT.resolve()), "state": source_state(ROOT, env),
            })
        if is_build_command(argv[1:]) and Path(cwd).resolve() == ROOT:
            from tls_dependencies import preflight
            preflight(tool, argv[1:], cwd, os.environ if env is None else env)
            automatic(os.environ if env is None else env, arguments=argv[1:])
            if not capture and input is None and argv[1] in {"build", "b", "check", "c", "clippy"} and not any(
                arg == "--" or arg.startswith(("--message-format", "--manifest-path", "--config", "--target-dir", "-Z", "-C"))
                for arg in argv[2:]
            ):
                from cargo_cache_usage import run
                return run(argv, root=ROOT, cwd=cwd, env=env, check=check)
    return subprocess.run(argv, cwd=cwd, env=env, check=check, text=True,
                          encoding="utf-8", errors="replace", capture_output=capture, input=input)


def source_state(root: Path, environment: dict[str, str]) -> str:
    """Invalidate build identity after commits and untracked source changes."""
    git = shutil.which("git", path=environment.get("PATH"))
    if git is None:
        return "unavailable"
    env = dict(environment, GIT_OPTIONAL_LOCKS="0")
    digest = hashlib.sha256()
    for arguments in (("rev-parse", "--verify", "HEAD"),
                      ("status", "--porcelain=v1", "-z", "--untracked-files=normal")):
        try:
            result = subprocess.run([git, "-C", str(root), *arguments], env=env,
                                    capture_output=True, check=False)
        except OSError:
            return "unavailable"
        if result.returncode:
            return "unavailable"
        digest.update(result.stdout)
        digest.update(b"\0")
    return digest.hexdigest()


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
        architecture = "arm64" if host_platform.machine().lower() in {"arm64", "aarch64"} else "x64"
        cmake_architecture = "arm64" if architecture == "arm64" else "x86_64"
        bins = [root / ".tools/llvm/bin", root / f".tools/cmake/cmake-4.4.4-windows-{cmake_architecture}/bin",
                root / ".tools/ninja", root / ".tools/nasm/nasm-3.02"]
        bins = [directory for directory in bins if directory.is_dir()]
        vswhere = Path(env.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Microsoft Visual Studio/Installer/vswhere.exe"
        vc = None
        sdk = Path(env.get("ProgramFiles(x86)", "C:/Program Files (x86)")) / "Windows Kits/10"
        if vswhere.is_file():
            installation = subprocess.run(
                [str(vswhere), "-latest", "-products", "*", "-requires",
                 "Microsoft.VisualStudio.Component.VC.Tools.ARM64" if architecture == "arm64" else "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"],
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
            # Select the native linker and target libraries for this Rust host.
            host_tools = "Hostarm64" if architecture == "arm64" else "Hostx64"
            bins += [vc / f"bin/{host_tools}/{architecture}", sdk / f"bin/{version}/{architecture}"]
            env["LIB"] = ";".join(map(str, [vc / f"lib/{architecture}", sdk / f"Lib/{version}/ucrt/{architecture}", sdk / f"Lib/{version}/um/{architecture}"]))
            env["INCLUDE"] = ";".join(map(str, [vc / "include", *[sdk / f"Include/{version}/{part}" for part in ("ucrt", "shared", "um", "winrt")]]))
        elif not vc and architecture == "x64":
            scope = Path("C:/Program Files/Microsoft Visual Studio/18/Community/SDK/ScopeCppSDK/vc15")
            if scope.is_dir():
                bins += [scope / "VC/bin", scope / "SDK/bin"]
                env["LIB"] = ";".join(map(str, [scope / "VC/lib", scope / "SDK/lib"]))
                env["INCLUDE"] = ";".join(map(str, [scope / "VC/include", *[scope / f"SDK/include/{part}" for part in ("ucrt", "shared", "um")]]))
        env.update(CC="clang-cl", CXX="clang-cl", AR="llvm-lib")
        if architecture == "arm64":
            env.setdefault("CFLAGS", "--target=aarch64-pc-windows-msvc")
            env.setdefault("CXXFLAGS", "--target=aarch64-pc-windows-msvc")
    else:
        env.setdefault("CC", "clang")
        env.setdefault("CXX", "clang++")
        env.setdefault("AR", "llvm-ar")
    separator = ";" if windows else os.pathsep
    path = [*map(str, bins), *env.get("PATH", "").split(separator)]
    if windows:
        # PowerShell setup and nested Python commands may configure the same
        # terminal repeatedly. cmd.exe drops PATH when it exceeds 8,191 chars.
        # Keep the first occurrence so native-tool precedence stays unchanged.
        unique = {}
        for directory in path:
            key = ntpath.normcase(ntpath.normpath(directory)) if directory else ""
            unique.setdefault(key, directory)
        path = list(unique.values())
    env["PATH"] = separator.join(path)
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
