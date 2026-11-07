"""Keep x64 build tools separate from Windows ARM64 target libraries."""
from __future__ import annotations

from pathlib import Path
import shutil

X64 = "x86_64-pc-windows-msvc"
ARM64 = "aarch64-pc-windows-msvc"


def target_libraries(environment, target):
    architecture = {X64: "x64", ARM64: "arm64"}[target]
    result = []
    for value in environment.get("LIB", "").split(";"):
        if not value:
            continue
        path = Path(value)
        if path.name.lower() not in {"x64", "arm64"}:
            raise ValueError(f"Unrecognized Windows target library directory: {path}")
        directory = path.parent / architecture
        if not directory.is_dir():
            raise ValueError(f"Missing {architecture} target libraries: {directory}; install the Visual Studio ARM64 C++ tools")
        result.append(directory)
    if len(result) != 3 or len(set(result)) != 3:
        raise ValueError("Expected separate MSVC, UCRT and Windows SDK target libraries")
    return result


def configure(environment, target, *, root, host):
    if host == target:
        return
    if (host, target) != (X64, ARM64):
        raise ValueError("Windows cross-compilation supports an x64 host and ARM64 output")
    libraries = target_libraries(environment, target)
    # LIB and PATH remain native: Cargo build scripts and generators run on x64.
    # Cargo applies these encoded rustflags only to the explicitly selected target.
    flags = environment.get("CARGO_ENCODED_RUSTFLAGS")
    flags = flags.split("\x1f") if flags else environment.get("RUSTFLAGS", "").split()
    for directory in libraries:
        argument = "-Lnative=" + str(directory)
        if argument not in flags:
            flags.append(argument)
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    environment["CARGO_BUILD_TARGET"] = target
    suffix = target.replace("-", "_")
    clang = shutil.which("clang-cl", path=environment["PATH"])
    archiver = shutil.which("llvm-lib", path=environment["PATH"])
    if not clang or not archiver:
        raise ValueError("Native clang-cl and llvm-lib are required for cross-compilation")
    environment.setdefault("CC_" + suffix, clang)
    environment.setdefault("CXX_" + suffix, clang)
    environment.setdefault("AR_" + suffix, archiver)
    environment.setdefault("CFLAGS_" + suffix, "--target=" + target)
    environment.setdefault("CXXFLAGS_" + suffix, "--target=" + target)
    # CMake's checks link executables before Rust links the final application.
    # Scope the toolchain to ARM64 so host-side CMake users remain native.
    directory = Path(root) / "artifacts/windows-cross"
    directory.mkdir(parents=True, exist_ok=True)
    toolchain = directory / "arm64.cmake"
    link_flags = " ".join('/LIBPATH:"' + str(path).replace("\\", "/") + '"' for path in libraries)
    text = '\n'.join((
        'set(CMAKE_SYSTEM_NAME Windows)',
        'set(CMAKE_SYSTEM_PROCESSOR ARM64)',
        'set(CMAKE_C_COMPILER "' + clang.replace("\\", "/") + '")',
        'set(CMAKE_CXX_COMPILER "' + clang.replace("\\", "/") + '")',
        'set(CMAKE_AR "' + archiver.replace("\\", "/") + '")',
        'set(CMAKE_C_COMPILER_TARGET aarch64-pc-windows-msvc)',
        'set(CMAKE_CXX_COMPILER_TARGET aarch64-pc-windows-msvc)',
        'set(CMAKE_EXE_LINKER_FLAGS_INIT [=[' + link_flags + ']=])',
        'set(CMAKE_SHARED_LINKER_FLAGS_INIT [=[' + link_flags + ']=])',
        '',
    ))
    if not toolchain.exists() or toolchain.read_text(encoding="utf-8") != text:
        toolchain.write_text(text, encoding="utf-8", newline="\n")
    environment["CMAKE_TOOLCHAIN_FILE_" + suffix] = str(toolchain)
