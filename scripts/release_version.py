"""Release track decisions and transactional workspace version updates."""
from dataclasses import dataclass
import json
from pathlib import Path
import re
from repo_tools import ROOT, command


def semver(version):
    if not isinstance(version, str) or not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("Use major.minor.patch without a v prefix or build metadata.")
    parts = tuple(map(int, version.split(".")))
    if any(part > 65535 for part in parts):
        raise ValueError("Windows installer version components must fit in 0-65535.")
    return parts


@dataclass(frozen=True)
class State:
    version: str
    channel: str
    release_type: str

    @classmethod
    def from_dict(cls, value):
        # Source files use camelCase; signed release manifests use PascalCase.
        data = {key.lower(): item for key, item in value.items()}
        version, channel = data["version"], data["channel"]
        semver(version)
        track = data.get("releasetype") or ("Canary" if channel == "Canary" else "Beta")
        if channel not in ("Canary", "Release") or track not in ("Canary", "Beta", "Stable") or ((track == "Canary") != (channel == "Canary")):
            raise ValueError("Release track does not match the source channel.")
        return cls(version, channel, track)

    def to_dict(self):
        return dict(version=self.version, channel=self.channel, releaseType=self.release_type)


def read_state(root=ROOT):
    return State.from_dict(json.loads((root / "release-version.json").read_text(encoding="utf-8-sig")))


def release_branch(version):
    major, minor, _ = semver(version)
    return f"release/{major}.{minor}"


def assert_source_branch(branch, version):
    expected = release_branch(version)
    if branch not in ("main", expected):
        raise ValueError(f"Version {version} requires main or the release-line branch {expected}.")


def resolve_type(state, branch, requested=None):
    assert_source_branch(branch, state.version)
    track = state.release_type if requested in (None, "", "Branch default") else requested
    if branch == "main":
        if state.channel != "Canary" or track != "Canary":
            raise ValueError("Main produces Canary releases.")
    elif state.channel != "Release" or track not in ("Beta", "Stable"):
        raise ValueError("A major.minor release branch produces Beta or Stable releases with a neutral Release source channel.")
    return track


def next_version(version):
    major, minor, patch = semver(version)
    if patch == 65535:
        raise ValueError("The Windows patch range is exhausted; choose the next minor or major explicitly.")
    return f"{major}.{minor}.{patch + 1}"


def main_line_decision(state, line):
    current, release = semver(state.version), semver(line)
    if current[:2] > release[:2]:
        return None
    if current[0] < release[0]:
        raise ValueError("Advance main to the new major explicitly; automation never increases a major version.")
    if release[1] == 65535:
        raise ValueError("The Windows minor range is exhausted; choose a new major explicitly.")
    return State(f"{release[0]}.{release[1] + 1}.0", "Canary", "Canary")


def post_release_decision(state, published, branch="main", published_type="Beta"):
    assert_source_branch(branch, published)
    current, released = semver(state.version), semver(published)
    if branch == "main":
        return None if current > released else State(next_version(published), "Canary", "Canary")
    assert_source_branch(branch, state.version)
    if current < released:
        raise ValueError("Release branch version is older than the published release; inspect it before advancing.")
    track = "Stable" if "Stable" in (state.release_type, published_type) else "Beta"
    if current > released:
        return None if state.channel == "Release" and state.release_type == track else State(state.version, "Release", track)
    version = next_version(published)
    assert_source_branch(branch, version)
    return State(version, "Release", track)


PIN = re.compile(r'(?m)(?P<prefix>^transmog[\w-]*\s*=\s*\{(?=[^\r\n}]*\bpath\s*=)[^\r\n}]*\bversion\s*=\s*")(?P<value>[^"]+)(?P<suffix>")')
WORKSPACE_VERSION = r'(?ms)(?P<prefix>^\[workspace\.package\]\r?\n(?:(?!^\[).)*?^version\s*=\s*")(?P<value>[^"]+)(?P<suffix>")'
JSON_VERSION = r'(?m)(?P<prefix>^  "version": ")(?P<value>[^"]+)(?P<suffix>")'
LOCK_ROOT_VERSION = r'(?ms)(?P<prefix>^    "": \{\r?\n(?:(?!^    \}).)*?^      "version": ")(?P<value>[^"]+)(?P<suffix>")'


def replace_version(text, pattern, current, new):
    regex = re.compile(pattern)
    matches = list(regex.finditer(text))
    if len(matches) != 1 or matches[0]["value"] != current:
        raise ValueError("Expected exactly one matching version field in the current file format.")
    return regex.sub(lambda match: match["prefix"] + new + match["suffix"], text)


def set_version(root, version=None, *, check=False, channel=None, release_type=None, run=command):
    root = root.resolve()
    state = read_state(root)
    version = state.version if check else version
    semver(version)
    track = release_type or ("Canary" if channel == "Canary" else "Beta" if channel == "Release" and state.release_type == "Canary" else state.release_type)
    selected_channel = "Canary" if track == "Canary" else "Release"
    selected = State.from_dict(dict(version=version, channel=selected_channel, releaseType=track))
    if channel and channel != selected_channel:
        raise ValueError("Source channel and release track disagree.")
    metadata = json.loads(run(["cargo", "metadata", "--manifest-path", root / "Cargo.toml", "--no-deps", "--format-version", "1", "--locked", "--offline"], cwd=root, capture=True).stdout)
    versions = {package["version"] for package in metadata["packages"]}
    if versions != {state.version}:
        raise ValueError("Cargo workspace versions differ from release-version.json.")
    config, ui, ui_lock = [root / name for name in ("apps/desktop/tauri.conf.json", "apps/desktop/ui/package.json", "apps/desktop/ui/package-lock.json")]
    documents = [json.loads(path.read_text(encoding="utf-8-sig")) for path in (config, ui, ui_lock)]
    if any(value != state.version for value in [*[doc["version"] for doc in documents], documents[-1]["packages"][""]["version"]]):
        raise ValueError("Tauri, Cargo, and desktop UI versions must agree before changing the release version.")
    manifests = list(dict.fromkeys([root / "Cargo.toml", *[Path(p["manifest_path"]) for p in metadata["packages"]], root / "fuzz/Cargo.toml"]))
    paths = [*manifests, config, ui, ui_lock, root / "release-version.json", root / "Cargo.lock", root / "fuzz/Cargo.lock"]
    if any(not path.resolve().is_relative_to(root) for path in paths):
        raise ValueError("A version file is outside the workspace.")
    original = {path: path.read_bytes() for path in paths}
    text = {path: data.decode("utf-8-sig") for path, data in original.items()}
    for path in manifests:
        if any(match["value"] != "=" + state.version for match in PIN.finditer(text[path])):
            raise ValueError(f"An internal dependency version differs in {path}")
    if check or selected == state:
        print(f"Committed release version: {state.version} ({state.release_type})")
        return state
    updated = {root / "release-version.json": json.dumps(selected.to_dict(), indent=2) + "\n"}
    if selected.version != state.version:
        updated.update({path: PIN.sub(lambda match: match["prefix"] + "=" + version + match["suffix"], text[path]) for path in manifests})
        updated[root / "Cargo.toml"] = replace_version(updated[root / "Cargo.toml"], WORKSPACE_VERSION, state.version, version)
        for path in (config, ui, ui_lock):
            updated[path] = replace_version(text[path], JSON_VERSION, state.version, version)
        updated[ui_lock] = replace_version(updated[ui_lock], LOCK_ROOT_VERSION, state.version, version)
    try:
        for path, contents in updated.items():
            path.write_bytes(contents.encode("utf-8"))
        if selected.version != state.version:
            for manifest in (root / "Cargo.toml", root / "fuzz/Cargo.toml"):
                run(["cargo", "update", "--manifest-path", manifest, "--workspace", "--offline"], cwd=root)
    except BaseException:
        # Include lockfiles and preserve exact original bytes, BOMs and line endings.
        for path, contents in original.items():
            path.write_bytes(contents)
        raise
    print(f"Set release version to {version} ({track}). Review git diff before committing.")
    return selected
