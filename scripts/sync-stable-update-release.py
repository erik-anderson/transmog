"""Point the stable feed at the highest stable version with signed update assets."""
import argparse
from github_release import api_root, request
from release_version import semver
from repo_tools import main_guard


def synchronize(repository, api=request):
    base = api_root(repository)
    best = None
    page = 1
    while True:
        releases = api(f"{base}/releases?per_page=100&page={page}")
        if not isinstance(releases, list):
            raise ValueError("GitHub did not return a release list.")
        for release in releases:
            if release.get("draft") or release.get("prerelease") or not release.get("tag_name", "").startswith("v"):
                continue
            version = release["tag_name"][1:]
            try:
                parts = semver(version)
            except ValueError:
                continue
            assets = [asset["name"] for asset in release["assets"]]
            if assets.count("latest.json") != 1 or assets.count(f"Transmog_{version}_x64-setup.exe.sig") != 1:
                continue
            if best is None or parts > semver(best["tag_name"][1:]):
                best = release
        if len(releases) < 100:
            break
        page += 1
    if best is None:
        print("No stable release with signed update assets exists yet.")
        return
    latest = api(base + "/releases/latest")
    if latest and latest["id"] == best["id"]:
        print(f"Stable update feed already selects {best['tag_name']}.")
        return
    api(f"{base}/releases/{best['id']}", method="PATCH", data={"make_latest": "true"})
    print(f"Stable update feed now selects {best['tag_name']}.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default="erik-anderson/transmog")
    synchronize(parser.parse_args().repository)


if __name__ == "__main__":
    main_guard(main)
