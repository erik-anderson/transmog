"""Reject an already published version or a reserved Canary release line."""
import argparse
import os
from pathlib import Path
from github_release import api_root, request
from release_version import read_state, release_branch, resolve_type
from repo_tools import ROOT, main_guard


def preflight(root, repository, branch, requested=None, api=request):
    state = read_state(root)
    track = resolve_type(state, branch, requested)
    base = api_root(repository)
    if branch == "main" and api(base + "/git/ref/heads/" + release_branch(state.version)):
        raise ValueError("This release line is reserved; advance main before releasing a Canary.")
    release = api(base + "/releases/tags/v" + state.version)
    if release and not release.get("draft"):
        raise ValueError(f"Release v{state.version} is already published; choose a new version before building.")
    return state, track


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository-root", type=Path, default=ROOT)
    parser.add_argument("--repository", default=os.environ.get("GITHUB_REPOSITORY", "erik-anderson/transmog"))
    parser.add_argument("--source-branch", default=os.environ.get("GITHUB_REF_NAME"))
    parser.add_argument("--requested-type", default=os.environ.get("REQUESTED_RELEASE_TYPE"))
    args = parser.parse_args()
    state, track = preflight(args.repository_root, args.repository, args.source_branch, args.requested_type)
    if os.environ.get("GITHUB_ENV"):
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8", newline="\n") as output:
            output.write(f"RELEASE_TYPE={track}\n")
    print(f"Release version {state.version} is available for {track}.")


if __name__ == "__main__":
    main_guard(main)
