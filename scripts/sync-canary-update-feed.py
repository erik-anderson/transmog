"""Publish the newest signed Canary manifest at the rolling updater URL."""
import argparse
import json
from github_release import api_root, request, upload_asset
from release_version import semver
from repo_tools import main_guard


def synchronize(repository, api=request, upload=upload_asset):
    base = api_root(repository)
    candidates = []
    page = 1
    while True:
        releases = api(f"{base}/releases?per_page=100&page={page}")
        if not isinstance(releases, list):
            raise ValueError("GitHub did not return a release list.")
        for release in releases:
            if release.get("draft") or not release.get("prerelease"):
                continue
            try:
                version = release["tag_name"][1:]
                parts = semver(version) if release["tag_name"].startswith("v") else None
            except (KeyError, ValueError):
                continue
            assets = {asset["name"]: asset for asset in release.get("assets", [])}
            manifest_asset = assets.get("release-manifest.json")
            update_asset = assets.get("latest.json")
            if not manifest_asset or not update_asset:
                continue
            manifest = json.loads(api(f"{base}/releases/assets/{manifest_asset['id']}", binary=True))
            if manifest.get("ReleaseType") != "Canary" or manifest.get("Version") != version:
                continue
            commit = manifest.get("Commit")
            if not isinstance(commit, str) or len(commit) != 40:
                continue
            update = api(f"{base}/releases/assets/{update_asset['id']}", binary=True)
            parsed = json.loads(update)
            if parsed.get("version") != version:
                continue
            candidates.append((parts, release, commit, update))
        if len(releases) < 100:
            break
        page += 1
    if not candidates:
        print("No published Canary release with a signed update manifest exists yet.")
        return
    _, newest, commit, payload = max(candidates, key=lambda item: item[0])

    rolling = api(f"{base}/releases/tags/canary")
    if rolling is None:
        api(f"{base}/git/refs", method="POST", data={"ref": "refs/tags/canary", "sha": commit})
        rolling = api(f"{base}/releases", method="POST", data={
            "tag_name": "canary", "target_commitish": commit, "name": "Canary update feed",
            "draft": False, "prerelease": True,
            "body": "Rolling signed updater manifest for the newest published Canary release.",
        })
    else:
        ref = api(f"{base}/git/ref/tags/canary")
        if ref["object"]["sha"] != commit:
            api(f"{base}/git/refs/tags/canary", method="PATCH", data={"sha": commit, "force": True})
    for asset in rolling.get("assets", []):
        if asset["name"] == "latest.json":
            api(f"{base}/releases/assets/{asset['id']}", method="DELETE")
    upload(rolling["upload_url"].split("{")[0], "latest.json", payload)
    print(f"Canary update feed now selects {newest['tag_name']}.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default="erik-anderson/transmog")
    synchronize(parser.parse_args().repository)


if __name__ == "__main__":
    main_guard(main)
