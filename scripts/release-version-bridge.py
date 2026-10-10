"""JSON bridge used by the remaining Windows PowerShell integration helpers."""
import json
from pathlib import Path
import sys
from release_version import State, assert_source_branch, main_line_decision, next_version, post_release_decision, read_state, release_branch, resolve_type, semver
from repo_tools import main_guard


def main():
    request = json.load(sys.stdin)
    operation = request["operation"]
    version = request.get("version")
    result = None
    if operation == "semver":
        semver(version)
        result = version
    elif operation == "state":
        state = read_state(Path(request["root"]))
        result = dict(Version=state.version, Channel=state.channel, ReleaseType=state.release_type, SemVer=state.version)
    elif operation == "next":
        result = next_version(version)
    elif operation == "branch":
        result = release_branch(version)
    elif operation == "assert-branch":
        assert_source_branch(request["branch"], version)
    elif operation == "resolve":
        result = resolve_type(State.from_dict(request["state"]), request["branch"], request.get("requested"))
    elif operation in ("main-decision", "post-decision"):
        state = State.from_dict(request["state"])
        decision = main_line_decision(state, version) if operation == "main-decision" else post_release_decision(state, version, request["branch"], request["publishedType"])
        result = dict(Bump=False, Reason="Version and release track are already current.") if decision is None else dict(Bump=True, Version=decision.version, Channel=decision.channel, ReleaseType=decision.release_type)
    else:
        raise ValueError("Unknown release-version bridge operation.")
    print(json.dumps(result))


if __name__ == "__main__":
    main_guard(main)
