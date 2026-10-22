"""Verify that app/CLI dependency closures cannot include the opt-in SDK."""
import json

from repo_tools import ROOT, command, development_environment, main_guard


def main():
    env = development_environment()
    metadata = json.loads(command(["cargo", "metadata", "--locked", "--format-version", "1"],
                                  env=env, capture=True).stdout)
    packages = {item["id"]: item for item in metadata["packages"]}
    nodes = {item["id"]: item for item in metadata["resolve"]["nodes"]}

    def is_sdk(package):
        return package["name"].startswith("transmog-sdk-") or "crubit" in package["name"]

    if any(is_sdk(packages[key]) for key in metadata["workspace_members"]):
        raise ValueError("SDK packages must not be root workspace members")
    for name in ("transmog", "transmog-desktop"):
        roots = [key for key, package in packages.items() if package["name"] == name
                 and package.get("source") is None]
        if len(roots) != 1:
            raise ValueError(f"Cannot identify application package {name}")
        seen, queue = set(), list(roots)
        while queue:
            key = queue.pop()
            if key in seen:
                continue
            seen.add(key)
            if is_sdk(packages[key]):
                raise ValueError(f"{name} depends on SDK package {packages[key]['name']}")
            queue.extend(dependency["pkg"] for dependency in nodes[key]["deps"])
        print(f"{name}: SDK-free dependency closure ({len(seen)} packages)")
    if metadata["workspace_root"].replace("\\", "/") != ROOT.as_posix():
        raise ValueError("Isolation check did not inspect the application workspace")


if __name__ == "__main__":
    main_guard(main)
