"""Maintain release branches only in the dedicated hosted workflow."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from github_release import api_root, request
from release_version import State, assert_source_branch, main_line_decision, post_release_decision, read_state, resolve_type, semver, set_version
from repo_tools import command, main_guard


def advance(event, event_name, root, environment, *, api=request, run=command):
    if environment.get("GITHUB_ACTIONS") != "true" or environment.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise ValueError("Version maintenance runs only in the dedicated hosted workflow.")
    root = root.resolve()
    creating = event_name == "create"
    manifest = None
    published_type = "Beta"
    published = None
    line = None
    def git(*arguments, check=True):
        return run(["git", *arguments], cwd=root, check=check, capture=True)
    if creating:
        branch = event.get("ref", "")
        if event.get("ref_type") != "branch" or not re.fullmatch(r"release/(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", branch):
            print("Not a major.minor release branch.")
            return
        line = branch.removeprefix("release/") + ".0"
        semver(line)
        description = "creation of " + branch
    else:
        release = event["release"]
        tag = release.get("tag_name", "")
        if release.get("draft") or not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
            print("No version update for a draft or legacy release.")
            return
        published = tag[1:]
        semver(published)
        assets = [asset for asset in release["assets"] if asset["name"] == "release-manifest.json"]
        if len(assets) != 1:
            raise ValueError("The published release must contain one release-manifest.json.")
        asset = assets[0]
        payload = api(f"{api_root(environment['GITHUB_REPOSITORY'])}/releases/assets/{asset['id']}", binary=True)
        if not isinstance(payload, bytes):
            raise ValueError("The release manifest is unavailable.")
        if asset.get("digest") and asset["digest"] != "sha256:" + hashlib.sha256(payload).hexdigest():
            raise ValueError("Release manifest digest differs from GitHub.")
        manifest = json.loads(payload)
        if manifest["Version"] != published or not re.fullmatch(r"[a-f0-9]{40}", manifest["Commit"]):
            raise ValueError("Release tag or commit is invalid.")
        branch = manifest["SourceBranch"]
        resolve_type(State.from_dict(manifest), branch, manifest["ReleaseType"])
        published_type = "Canary" if branch == "main" else "Beta" if release.get("prerelease") else "Stable"
        description = tag
        git("fetch", "--no-tags", "origin", "refs/tags/" + tag)
        if git("rev-parse", "FETCH_HEAD^{commit}").stdout.strip() != manifest["Commit"]:
            raise ValueError("Published tag differs from installer source commit.")
    git("config", "user.name", "github-actions[bot]")
    git("config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com")

    def update(target):
        for _ in range(3):
            fetch = git("fetch", "--no-tags", "origin", f"refs/heads/{target}:refs/remotes/origin/{target}", check=False)
            if fetch.returncode:
                if "couldn't find remote ref" in fetch.stderr:
                    print(f"{target} was deleted; skipping its update.")
                    return
                raise RuntimeError(f"Could not fetch {target}: {fetch.stderr.strip()}")
            if git("status", "--porcelain").stdout.strip():
                raise ValueError("The hosted version checkout is not clean.")
            git("checkout", "-B", "prepare-next-version", f"refs/remotes/origin/{target}")
            state = read_state(root)
            if target != "main":
                assert_source_branch(target, state.version)
                if not creating:
                    git("merge-base", "--is-ancestor", manifest["Commit"], "HEAD")
            if creating and target == "main":
                decision = main_line_decision(state, line)
            elif creating:
                decision = State(state.version, "Release", "Beta" if state.release_type == "Canary" else state.release_type) if state.channel != "Release" else None
            else:
                decision = post_release_decision(state, published, target, published_type)
            if decision is None:
                print(f"{target}: version and track are already current.")
                return
            if decision.version != state.version:
                run(["cargo", "fetch", "--locked"], cwd=root)
                run(["cargo", "fetch", "--manifest-path", "fuzz/Cargo.toml", "--locked"], cwd=root)
            set_version(root, decision.version, release_type=decision.release_type, run=run)
            git("add", "--update")
            git("commit", "-m", f"Prepare {decision.version} {decision.release_type} on {target} after {description}")
            push = git("push", "origin", f"HEAD:refs/heads/{target}", check=False)
            if push.returncode == 0:
                print(f"Updated {target} to {decision.version} {decision.release_type}.")
                summary = environment.get("GITHUB_STEP_SUMMARY")
                if summary:
                    with open(summary, "a", encoding="utf-8", newline="\n") as output:
                        output.write(f"{description}: **{target}** is now **{decision.version} {decision.release_type}**.\n")
                return
        raise RuntimeError(f"Could not push {target} after three attempts; inspect concurrent changes or branch protection.")

    failures = []
    for target in dict.fromkeys([branch, "main"] if creating else [branch]):
        try:
            update(target)
        except (RuntimeError, ValueError, OSError, subprocess.CalledProcessError) as error:
            failures.append(f"{target}: {error}")
    if failures:
        raise RuntimeError("\n".join(failures))


def main():
    argparse.ArgumentParser(description=__doc__).parse_args()
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise ValueError("Version maintenance runs only in the dedicated hosted workflow.")
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8-sig"))
    advance(event, os.environ["GITHUB_EVENT_NAME"], Path.cwd(), dict(os.environ))


if __name__ == "__main__":
    main_guard(main)
