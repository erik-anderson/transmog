"""Check development tools, print environment JSON, or run a configured child."""
import argparse
import json
import os
import sys
from repo_tools import command, development_environment, executable, main_guard


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--emit-env", action="store_true")
    parser.add_argument("--run", nargs=argparse.REMAINDER)
    # Python 3.11 argparse may split an option's REMAINDER at a literal --.
    # Once --run appears, its entire suffix belongs to the child command.
    argv = sys.argv[1:]
    if "--run" in argv:
        index = argv.index("--run")
        args = parser.parse_args(argv[:index])
        args.run = argv[index + 1:]
    else:
        args = parser.parse_args(argv)
    if args.run is not None and not args.run:
        parser.error("--run requires a command")
    env = development_environment()
    if args.emit_env:
        from build_cache import automatic
        automatic(env)
        print(json.dumps({key: value for key, value in env.items() if os.environ.get(key) != value}))
    elif args.run:
        result = command(args.run, env=env, check=False)
        raise SystemExit(result.returncode)
    else:
        print(f"Python {sys.version.split()[0]}: {sys.executable}")
        for tool in ("rustc", "cargo", "clang", "cmake", "ninja", "nasm"):
            print(f"{tool:10} {executable(tool, env)}")
        print("Use --run COMMAND [ARGS...] to run with the configured native environment.")


if __name__ == "__main__":
    main_guard(main)
