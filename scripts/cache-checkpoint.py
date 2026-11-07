"""Report actual cache availability instead of a best-effort upload result."""
import argparse
import json
import os
from pathlib import Path

from repo_tools import main_guard


def flag(name, default="false"):
    value = os.environ.get(name, default)
    if value not in {"", "true", "false"}:
        raise ValueError(f"{name} must be true, false, or an empty cache result")
    return value == "true"


def report(label, key, *, enabled, restored, existing, available,
           report_path, summary_path=None, output_path=None):
    if not enabled:
        status, message = "disabled", "caching disabled; no lookup or upload requested"
    elif not available:
        status, message = "unavailable", "exact cache key is unavailable after the upload attempt"
    elif restored:
        status, message = "reused", "restored existing checkpoint; upload skipped"
    elif existing:
        status, message = "reused", "another producer already saved this checkpoint; duplicate upload skipped"
    else:
        status, message = "available", "exact cache key confirmed available after the upload attempt"
    result = {"label": label, "key": key, "status": status, "message": message}
    report_path = Path(report_path)
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"{label}: {message}.", flush=True)
    if summary_path:
        with Path(summary_path).open("a", encoding="utf-8") as summary:
            summary.write(f"\n**{label} cache: {status}.** {message}.\n")
    if output_path:
        with Path(output_path).open("a", encoding="utf-8") as output:
            output.write(f"status={status}\n")
    if status == "unavailable":
        raise RuntimeError(f"{label} cache checkpoint failed: {message}; see the upload diagnostics")
    return result


def main():
    argparse.ArgumentParser(description=__doc__).parse_args()
    report(os.environ["CHECKPOINT_LABEL"], os.environ["CHECKPOINT_KEY"],
           enabled=flag("CHECKPOINT_ENABLED", "true"),
           restored=flag("CHECKPOINT_RESTORED"),
           existing=flag("CHECKPOINT_EXISTING"),
           available=flag("CHECKPOINT_AVAILABLE"),
           report_path=os.environ["CHECKPOINT_REPORT"],
           summary_path=os.environ.get("GITHUB_STEP_SUMMARY"),
           output_path=os.environ.get("GITHUB_OUTPUT"))


if __name__ == "__main__":
    main_guard(main)
