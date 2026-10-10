"""Run real proxy interoperability against isolated Docker origin fixtures."""
import argparse
import socket
import tempfile
from pathlib import Path
from repo_tools import ROOT, cargo_output, command, development_environment, executable, main_guard, native_name


def free_tcp_udp_port():
    for _ in range(20):
        with socket.socket() as tcp, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
            tcp.bind(("127.0.0.1", 0))
            port = tcp.getsockname()[1]
            try:
                udp.bind(("127.0.0.1", port))
                return port
            except OSError:
                pass
    raise RuntimeError("Could not reserve a matching loopback TCP/UDP port for HTTP/3.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-browser-install", action="store_true")
    parser.add_argument("--skip-dependency-install", action="store_true")
    args = parser.parse_args()
    env = development_environment()
    docker = executable("docker", env)
    curl = executable("curl", env)
    project_root = ROOT / "e2e/playwright"
    interop_root = ROOT / ".local/interop"
    interop_root.mkdir(parents=True, exist_ok=True)
    command(["cargo", "build", "--release", "--locked", "-p", "transmog", "-p", "transmog-script-host", "-p", "transmog-preview-worker"], env=env)
    output = cargo_output(env) / "release"
    binary = output / native_name("transmog-cli")
    with tempfile.TemporaryDirectory(prefix="run-", dir=interop_root) as temporary:
        run_root = Path(temporary)
        project = "transmog-interop-" + run_root.name
        compose = [docker, "compose", "--project-name", project, "--file", ROOT / "e2e/interop/compose.yaml"]
        ca, key = run_root / "ca.pem", run_root / "ca.key"
        command([binary, "ca", "generate", "--cert", ca, "--key", key, "--name", project], env=env)
        command([binary, "ca", "issue", "--ca-cert", ca, "--ca-key", key, "--identity", "127.0.0.1", "--cert", run_root / "origin.pem", "--key", run_root / "origin.key", "--days", "7"], env=env)
        assets = run_root / "encoded"
        command(["node", ROOT / "e2e/interop/generate-encoded-fixtures.mjs", assets], env=env)
        port = free_tcp_udp_port()
        env.update(TRANSMOG_INTEROP_ASSETS_DIR=str(assets), TRANSMOG_INTEROP_TLS_DIR=str(run_root), TRANSMOG_INTEROP_TLS_PORT=str(port))
        try:
            command([*compose, "up", "--detach", "--pull", "missing", "--wait", "--wait-timeout", "120"], env=env)
            env.update(TRANSMOG_BIN=str(binary), TRANSMOG_TEST_CA_CERT=str(ca), TRANSMOG_TEST_CA_KEY=str(key),
                       TRANSMOG_CURL=curl, TRANSMOG_CADDY_URL=f"https://127.0.0.1:{port}/", TRANSMOG_INTEROP="1",
                       TRANSMOG_SCRIPT_HOST=str(output / native_name("transmog-script-host")),
                       TRANSMOG_PREVIEW_WORKER=str(output / native_name("transmog-preview-worker")))
            for service in ("nginx", "apache"):
                binding = command([*compose, "port", service, "80"], env=env, capture=True).stdout.splitlines()[0].strip()
                if not binding.startswith("127.0.0.1:") or not binding.split(":")[-1].isdecimal():
                    raise RuntimeError(f"Unexpected loopback port binding for {service}: {binding}")
                env[f"TRANSMOG_{service.upper()}_URL"] = f"http://{binding}/"
            command(["cargo", "test", "--release", "--locked", "-p", "transmog-app", "--test", "headless_product", "--", "--ignored", "--nocapture"], env=env)
            if not args.skip_dependency_install:
                command(["npm", "ci"], cwd=project_root, env=env)
            if not args.skip_browser_install:
                command(["node", "node_modules/playwright/cli.js", "install", "chromium"], cwd=project_root, env=env)
            command(["npm", "run", "test:interop"], cwd=project_root, env=env)
        except Exception:
            command([*compose, "logs", "--no-color"], env=env, check=False)
            raise
        finally:
            result = command([*compose, "down", "--volumes", "--remove-orphans"], env=env, check=False)
            if result.returncode:
                raise RuntimeError(f"Docker Compose cleanup failed ({result.returncode}); retained project: {project}")


if __name__ == "__main__":
    main_guard(main)
