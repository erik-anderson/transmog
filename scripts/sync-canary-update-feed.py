"""Publish the newest Canary download links and signed rolling updater manifest."""
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
            if release.get("draft") or not release.get("prerelease") or not release.get("tag_name", "").startswith("v"):
                continue
            try:
                version = release["tag_name"][1:]
                parts = semver(version)
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
            candidates.append((parts, release, commit, update, parsed))
        if len(releases) < 100:
            break
        page += 1
    if not candidates:
        print("No published Canary release with a signed update manifest exists yet.")
        return
    _, newest, commit, payload, update = max(candidates, key=lambda item: item[0])
    tag = newest["tag_name"]
    published_urls = {asset.get("browser_download_url") for asset in newest.get("assets", [])
                      if asset.get("state") == "uploaded"}
    rows = []
    for platform, label, architecture in (
        ("windows-x86_64-nsis", "x64 (Intel / AMD)", "x64"),
        ("windows-aarch64-nsis", "ARM64", "ARM64"),
    ):
        entry = update.get("platforms", {}).get(platform)
        if entry is None:
            continue
        url = entry.get("url")
        if not isinstance(url, str) or url not in published_urls:
            raise ValueError(f"The newest Canary is missing its published Windows {architecture} installer.")
        rows.append(f"| {label} | **[Download for Windows {architecture}]({url})** |")
    if not rows:
        raise ValueError("The newest Canary has no Windows installer downloads.")
    details = {
        "tag_name": "canary", "target_commitish": commit, "name": f"Latest Canary: {tag}",
        "draft": False, "prerelease": True, "make_latest": "false",
        "body": "\n".join([
            "## Download for Windows", "",
            f"**Transmog {tag} Canary** is a prerelease build for testing.", "",
            "| Windows PC | Signed installer |",
            "| --- | --- |",
            *rows,
            "", "The installer includes the desktop app and command-line tool.", "",
            f"[Release notes, standalone CLI, checksums, symbols, and verification reports](https://github.com/{repository}/releases/tag/{tag}).",
            "", "This page follows the newest published Canary. Its `latest.json` asset is used by automatic updates.",
        ]),
    }

    rolling = api(f"{base}/releases/tags/canary")
    # A release can be removed without deleting its Git tag (or vice versa).
    # Reconcile the two resources independently so retries recover either state.
    ref = api(f"{base}/git/ref/tags/canary")
    if ref is None:
        api(f"{base}/git/refs", method="POST", data={"ref": "refs/tags/canary", "sha": commit})
    elif ref["object"]["sha"] != commit:
        api(f"{base}/git/refs/tags/canary", method="PATCH", data={"sha": commit, "force": True})
    if rolling is None:
        rolling = api(f"{base}/releases", method="POST", data=details)
    elif any(rolling.get(key) != details[key] for key in ("name", "body", "draft", "prerelease")):
        api(f"{base}/releases/{rolling['id']}", method="PATCH", data=details)
    for asset in rolling.get("assets", []):
        if asset["name"] == "latest.json":
            existing = api(f"{base}/releases/assets/{asset['id']}", binary=True)
            if existing == payload:
                print(f"Canary update feed already selects {newest['tag_name']}.")
                return
            api(f"{base}/releases/assets/{asset['id']}", method="DELETE")
    upload(rolling["upload_url"].split("{")[0], "latest.json", payload)
    print(f"Canary update feed now selects {newest['tag_name']}.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default="erik-anderson/transmog")
    synchronize(parser.parse_args().repository)


if __name__ == "__main__":
    main_guard(main)
