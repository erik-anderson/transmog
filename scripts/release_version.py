"""Release track decisions and transactional workspace version updates."""
from dataclasses import dataclass
import json
from pathlib import Path
import re
import tomllib
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
CARGO_LOCK_PACKAGE = re.compile(r'(?ms)^\[\[package\]\]\r?\n.*?(?=^\[|\Z)')
CARGO_PACKAGE_VERSION = r'(?m)(?P<prefix>^version = ")(?P<value>[^"]+)(?P<suffix>")'
CARGO_LOCK_DEPENDENCY = re.compile(r'(?m)^(?P<prefix>[ \t]+")(?P<id>[^"\r\n]+)(?P<suffix>",\r?)$')
SDK_CMAKE_VERSION = re.compile(r'(?m)(?P<prefix>^project\(TransmogSDK VERSION )(?P<value>\S+)(?P<suffix> )')
SDK_MANAGED_VERSION = re.compile(r'(?m)(?P<prefix>^[ \t]+<Version>)(?P<value>[^<\r\n]+)(?P<suffix></Version>)')
SDK_MANAGED_ASSEMBLY = re.compile(r'(?m)^[ \t]+<AssemblyVersion>')
SDK_MANAGED_FILE_VERSION = re.compile(r'(?m)(?P<prefix>^[ \t]+<FileVersion>)(?P<value>[^<\r\n]+)(?P<suffix></FileVersion>)')
SDK_MANAGED_FRAMEWORK = re.compile(r'(?m)^(?P<indent>[ \t]+)<TargetFramework>[^\r\n]+(?P<newline>\r?\n)')


def replace_version(text, pattern, current, new):
    regex = re.compile(pattern)
    matches = list(regex.finditer(text))
    if len(matches) != 1 or matches[0]["value"] != current:
        raise ValueError("Expected exactly one matching version field in the current file format.")
    return regex.sub(lambda match: match["prefix"] + new + match["suffix"], text)


def sync_sdk_lock(text, workspace_names, required_names, version):
    packages = tomllib.loads(text).get("package", [])
    missing = required_names - {p["name"] for p in packages if "source" not in p}
    if missing:
        raise ValueError(f"SDK lockfile is missing local packages {', '.join(sorted(missing))}. Regenerate it with Cargo.")
    versioned_dependencies = {}
    entries = iter(packages)

    def update_package(match):
        block = match[0]
        package = next(entries)
        name, old = package["name"], package["version"]
        if name not in workspace_names or "source" in package or old == version:
            return block
        versioned_dependencies[f"{name} {old}"] = f"{name} {version}"
        return replace_version(block, CARGO_PACKAGE_VERSION, old, version)

    # The SDK resolves independently and may only use a subset of the desktop
    # workspace. Keep its existing graph and update local workspace identities.
    if sum(1 for _ in CARGO_LOCK_PACKAGE.finditer(text)) != len(packages):
        raise ValueError("SDK lockfile formatting differs from Cargo's generated format.")
    updated = CARGO_LOCK_PACKAGE.sub(update_package, text)
    return CARGO_LOCK_DEPENDENCY.sub(
        lambda match: match["prefix"] + versioned_dependencies.get(match["id"], match["id"]) + match["suffix"],
        updated,
    )


def workspace_packages(root, manifest, run):
    result = run(["cargo", "metadata", "--manifest-path", manifest, "--no-deps", "--format-version", "1",
                  "--locked", "--offline"], cwd=root, capture=True)
    return json.loads(result.stdout)["packages"]


def sync_sdk_managed_project(text, version):
    properties = []
    match = SDK_MANAGED_VERSION.search(text)
    if match:
        text = replace_version(text, SDK_MANAGED_VERSION, match["value"], version)
    else:
        properties.append(f"<Version>{version}</Version>")
    if not SDK_MANAGED_ASSEMBLY.search(text):
        # Preserve the original CLR identity when updating older release branches.
        properties.append("<AssemblyVersion>1.0.0.0</AssemblyVersion>")
    file_version = SDK_MANAGED_FILE_VERSION.search(text)
    if file_version:
        text = replace_version(text, SDK_MANAGED_FILE_VERSION, file_version["value"], "$(Version).0")
    else:
        properties.append("<FileVersion>$(Version).0</FileVersion>")
    if not properties:
        return text
    updated, count = SDK_MANAGED_FRAMEWORK.subn(
        lambda item: "".join(f'{item["indent"]}{property_line}{item["newline"]}' for property_line in properties) + item[0],
        text,
    )
    if count != 1:
        raise ValueError("Cannot locate the SDK managed project property group.")
    return updated


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
    packages = workspace_packages(root, root / "Cargo.toml", run)
    versions = {package["version"] for package in packages}
    if versions != {state.version}:
        raise ValueError("Cargo workspace versions differ from release-version.json.")
    config, ui, ui_lock = [root / name for name in ("apps/desktop/tauri.conf.json", "apps/desktop/ui/package.json", "apps/desktop/ui/package-lock.json")]
    documents = [json.loads(path.read_text(encoding="utf-8-sig")) for path in (config, ui, ui_lock)]
    if any(value != state.version for value in [*[doc["version"] for doc in documents], documents[-1]["packages"][""]["version"]]):
        raise ValueError("Tauri, Cargo, and desktop UI versions must agree before changing the release version.")
    manifests = list(dict.fromkeys([root / "Cargo.toml", *[Path(p["manifest_path"]) for p in packages], root / "fuzz/Cargo.toml"]))
    paths = [*manifests, config, ui, ui_lock, root / "release-version.json", root / "Cargo.lock", root / "fuzz/Cargo.lock"]
    sdk_manifest = root / "sdk/Cargo.toml"
    sdk_lock = root / "sdk/Cargo.lock"
    sdk_cmake = root / "sdk/CMakeLists.txt"
    sdk_managed = root / "sdk/managed/Transmog.Sdk.csproj"
    if sdk_manifest.is_file():
        paths.extend((sdk_manifest, sdk_lock, *(path for path in (sdk_cmake, sdk_managed) if path.is_file())))
    if any(not path.resolve().is_relative_to(root) for path in paths):
        raise ValueError("A version file is outside the workspace.")
    original = {path: path.read_bytes() for path in paths}
    text = {path: data.decode("utf-8-sig") for path, data in original.items()}
    for path in manifests:
        if any(match["value"] != "=" + state.version for match in PIN.finditer(text[path])):
            raise ValueError(f"An internal dependency version differs in {path}")
    updated = {}
    if sdk_lock in text:
        sdk_packages = workspace_packages(root, sdk_manifest, run)
        sdk_version = tomllib.loads(text[sdk_manifest])["workspace"]["package"]["version"]
        if {p["version"] for p in sdk_packages} != {sdk_version}:
            raise ValueError("SDK crate versions differ from the SDK workspace version.")
        product_names = {p["name"] for p in packages}
        sdk_names = {p["name"] for p in sdk_packages}
        sdk_product_dependencies = {d["name"] for p in sdk_packages for d in p["dependencies"]
                                    if d.get("path") and d["name"] in product_names}
        sdk_updates = {
            sdk_manifest: replace_version(text[sdk_manifest], WORKSPACE_VERSION, sdk_version, version),
            sdk_lock: sync_sdk_lock(text[sdk_lock], product_names | sdk_names,
                                    sdk_names | sdk_product_dependencies, version),
        }
        if sdk_cmake in text:
            cmake_version = SDK_CMAKE_VERSION.search(text[sdk_cmake])
            if not cmake_version:
                raise ValueError("Cannot locate the SDK CMake project version.")
            sdk_updates[sdk_cmake] = replace_version(text[sdk_cmake], SDK_CMAKE_VERSION, cmake_version["value"], version)
        if sdk_managed in text:
            sdk_updates[sdk_managed] = sync_sdk_managed_project(text[sdk_managed], version)
        updated.update({path: value for path, value in sdk_updates.items() if value != text[path]})
        if check and updated:
            raise ValueError(f"SDK versions differ from {state.version}. Run set-release-version.py --version {state.version} to synchronize them.")
    if check or (selected == state and not updated):
        print(f"Committed release version: {state.version} ({state.release_type})")
        return state
    if selected != state:
        updated[root / "release-version.json"] = json.dumps(selected.to_dict(), indent=2) + "\n"
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
        if sdk_manifest in updated and {p["version"] for p in workspace_packages(root, sdk_manifest, run)} != {version}:
            raise ValueError("SDK crates must follow the SDK workspace version.")
    except BaseException:
        # Include lockfiles and preserve exact original bytes, BOMs and line endings.
        for path, contents in original.items():
            path.write_bytes(contents)
        raise
    if selected == state:
        print(f"Synchronized the SDK version with {version}.")
    else:
        print(f"Set release version to {version} ({track}). Review git diff before committing.")
    return selected
