"""Run the complete portable local validation gate."""
import argparse
import sys
from repo_tools import ROOT, command, development_environment, main_guard


def main():
    argparse.ArgumentParser(description=__doc__).parse_args()
    env = development_environment()
    command(["npm", "run", "check"], env=env)
    command([sys.executable, ROOT / "scripts/release-policy.py", "--prepare",
             "--snapshot", ROOT / "artifacts/release-policy/local-snapshot",
             "--output", ROOT / "artifacts/release-policy/local-check"], env=env)
    command([sys.executable, "-m", "unittest", "discover", "-s", "scripts/tests"], env=env)
    command([sys.executable, ROOT / "scripts/vendor-boringssl-pki.py", "--check"], env=env)
    command([sys.executable, ROOT / "scripts/swc_dependencies.py"], env=env)
    command([sys.executable, ROOT / "scripts/test-chromium-psl.py"], env=env)
    command([sys.executable, ROOT / "scripts/update-chromium-certificate-policy.py", "--check"], env=env)
    command([sys.executable, ROOT / "scripts/test-chromium-certificate-policy.py"], env=env)
    command(["cargo", "fmt", "--all", "--", "--check"], env=env)
    command(["cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings"], env=env)
    command(["cargo", "test", "--workspace", "--all-features", "--locked"], env=env)
    command([sys.executable, ROOT / "scripts/check-crypto-graph.py"], env=env)
    command([sys.executable, ROOT / "scripts/generate-supply-chain-artifacts.py"], env=env)


if __name__ == "__main__":
    main_guard(main)
