"""Compare local Chromium and production Transmog traffic over H1.1, H2 and H3."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import secrets
import shutil
import subprocess
import tempfile
from browser_fingerprint import SynCapture, comparison_fields, differences, flatten, measure, write_report
from browser_tools import build_browser_image
from docker_tools import container_arguments
from repo_tools import ROOT, command, main_guard


def inside(args):
    directory = Path(args.output_dir)
    comparisons, versions = [], set()
    with SynCapture() as capture:
        for protocol in args.protocols:
            for sample in range(args.samples):
                signals = {}
                for leg in ("direct", "proxied"):
                    run = directory / f"{protocol}-{sample}-{leg}"
                    run.mkdir()
                    subprocess.run(["/usr/local/bin/node", "/opt/pqc-browser/browser-fingerprint.mjs", args.binary,
                                    str(run), protocol, leg, args.channel, secrets.token_hex(16)],
                                   check=True, timeout=120)
                    browser = json.loads((run / "browser.json").read_text())
                    versions.add(browser["browser"])
                    record = json.loads((run / browser["originRecord"]).read_text())
                    signals[leg] = {**measure(record, capture), "browser_evidence":browser}
                    (run / "signals.json").write_text(json.dumps(signals[leg], indent=2) + "\n")
                delta = differences(comparison_fields(signals["direct"]), comparison_fields(signals["proxied"]))
                comparisons.append({"protocol":protocol, "sample":sample, "differences":delta, **signals})
                print(f"{protocol} sample {sample}: {len(delta)} differing fields", flush=True)
        (directory / "tcp-syn.json").write_text(json.dumps(capture.records, indent=2) + "\n")
    if len(versions) != 1:
        raise ValueError("Browser version changed during the comparison")
    variation = {}
    for protocol in args.protocols:
        for leg in ("direct", "proxied"):
            samples = [flatten(comparison_fields(c[leg])) for c in comparisons if c["protocol"] == protocol]
            keys = set().union(*(s.keys() for s in samples))
            variation[f"{protocol}.{leg}"] = [key for key in sorted(keys) if len({json.dumps(s.get(key), sort_keys=True) for s in samples}) > 1]
    result = {"schema":1, "environment":{"browser":versions.pop(), "platform":platform.platform(), "channel":args.channel,
              "scope":"Fresh connections, H1.1/H2/H3; loopback only; no resumption or real DNS ECH; origin declines ALPS",
              "normalization":"GREASE codes, ephemeral key bytes, TLS random/session IDs, QUIC CIDs, local listener ports; raw records retained",
              "trust":"Ephemeral private fixture roots; no OS installation. Custom Chromium transport recipe uses isolated OS-mode trust and bundled ID metadata.",
              "metadata":json.loads((directory / "source.json").read_text())},
              "within_client_variation":variation, "comparisons":comparisons}
    write_report(directory, result)
    if args.require_match and any(c["differences"] for c in comparisons):
        raise SystemExit(2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--protocols", nargs="+", choices=("h1", "h2", "h3"), default=["h1", "h2", "h3"])
    parser.add_argument("--channel", choices=("chrome",), default="chrome", help=argparse.SUPPRESS)
    parser.add_argument("--output-dir")
    parser.add_argument("--refresh-browser", action="store_true")
    parser.add_argument("--require-match", action="store_true", help="Exit 2 after writing reports when normalized differences exist")
    parser.add_argument("--inside", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--binary", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not 1 <= args.samples <= 10 or len(set(args.protocols)) != len(args.protocols):
        parser.error("Choose 1–10 samples and distinct protocols")
    if args.inside:
        if not args.binary or not args.output_dir:
            parser.error("Internal execution requires --binary and --output-dir")
        try:
            inside(args)
        except Exception as error:
            (Path(args.output_dir) / "failure.json").write_text(json.dumps({"status":"failed", "error":str(error)}, indent=2) + "\n")
            raise
        return
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + secrets.token_hex(3)
    output = Path(args.output_dir).resolve() if args.output_dir else ROOT / ".local/chromium-comparison" / stamp
    if "," in str(output):
        parser.error("Docker evidence mounts require a path without commas")
    output.mkdir(parents=True, exist_ok=False)
    revision = command(["git", "rev-parse", "HEAD"], capture=True).stdout.strip()
    diff = command(["git", "diff", "HEAD"], capture=True).stdout
    source = {"revision":revision, "tracked_diff_sha256":hashlib.sha256(diff.encode()).hexdigest(),
              "harness_sources":{str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [Path(__file__), ROOT / "scripts/browser_fingerprint.py", ROOT / "scripts/browser_tools.py", ROOT / "e2e/fingerprint/Dockerfile", ROOT / "e2e/playwright/support/browser-fingerprint.mjs", *sorted((ROOT / "crates/proxy-runtime/examples").glob("fingerprint/*.rs")), ROOT / "crates/proxy-runtime/examples/browser-fingerprint.rs"]}}
    (output / "source.json").write_text(json.dumps(source, indent=2) + "\n")
    docker = build_browser_image(refresh=args.refresh_browser)
    image = "transmog-browser-fingerprint"
    with tempfile.TemporaryDirectory(prefix="transmog-fingerprint-image-") as temporary:
        context = Path(temporary)
        shutil.copyfile(ROOT / "e2e/fingerprint/Dockerfile", context / "Dockerfile")
        shutil.copyfile(ROOT / "e2e/playwright/support/browser-fingerprint.mjs", context / "browser-fingerprint.mjs")
        command([docker, "build", "--tag", image, context])
    image = command([docker, "image", "inspect", "--format", "{{.Id}}", image], capture=True).stdout.strip()
    source["image_id"] = image
    mounts = ("type=volume,source=transmog-linux-cargo-registry,target=/usr/local/cargo/registry",
              "type=volume,source=transmog-linux-cargo-git,target=/usr/local/cargo/git",
              "type=volume,source=transmog-linux-target,target=/work-target")
    build = container_arguments(docker, image, mounts=mounts,
        environment=("CFLAGS=", "CXXFLAGS=", "CARGO_TARGET_DIR=/work-target", "RUST_MIN_STACK=16777216"))
    command([*build, "python3", "scripts/dev-env.py", "--check"])
    command([*build, "python3", "scripts/dev-env.py", "--run", "cargo", "build", "--locked", "-p", "transmog-runtime", "--example", "browser-fingerprint"])
    if command(["git", "rev-parse", "HEAD"], capture=True).stdout.strip() != revision or command(["git", "diff", "HEAD"], capture=True).stdout != diff:
        raise RuntimeError("Repository changed while building; rerun against a stable checkout")
    for name, expected in source["harness_sources"].items():
        if hashlib.sha256((ROOT / name).read_bytes()).hexdigest() != expected:
            raise RuntimeError("Harness changed while building; rerun against a stable checkout")
    (output / "source.json").write_text(json.dumps(source, indent=2) + "\n")
    run = container_arguments(docker, image, mounts=(*mounts, f"type=bind,source={output},target=/evidence"))
    # Runtime receives no network except its private loopback. NET_RAW is scoped
    # to this container; neither host networking nor privileged mode is needed.
    run[2:2] = ["--network", "none", "--cap-drop", "ALL", "--cap-add", "NET_RAW", "--shm-size", "1g"]
    argv = [*run, "python3", "scripts/compare-chromium.py", "--inside", "--binary", "/work-target/debug/examples/browser-fingerprint",
            "--output-dir", "/evidence", "--samples", str(args.samples), "--protocols", *args.protocols]
    if args.require_match: argv.append("--require-match")
    print(f"Comparison evidence: {output}", flush=True)
    result = command(argv, check=False)
    if (output / "report.html").is_file():
        print(f"Report: {output / 'report.html'}", flush=True)
    else:
        print(f"Comparison failed; inspect fixture logs in {output}", flush=True)
    if result.returncode: raise SystemExit(result.returncode)


if __name__ == "__main__":
    main_guard(main)
