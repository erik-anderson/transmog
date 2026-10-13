"""Check each target's production graph for one BoringSSL family."""
import json
from repo_tools import command, main_guard

TARGETS = ("x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu", "aarch64-apple-darwin")
FORBIDDEN = {"openssl", "openssl-sys", "native-tls", "rustls", "rustls-webpki", "webpki"}


def check_graph(metadata, target):
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    # The desktop configures the SDK's existing reqwest client. Exempt only that
    # exact package instance; a second version or any other consumer is checked.
    updater_clients = {}
    for key, node in nodes.items():
        if packages[key]["name"] != "transmog-desktop":
            continue
        clients = updater_clients[key] = set()
        for dep in node["deps"]:
            if packages[dep["pkg"]]["name"] == "tauri-plugin-updater" and any(
                kind["kind"] in (None, "normal") for kind in dep["dep_kinds"]
            ):
                clients.update(
                    client["pkg"] for client in nodes[dep["pkg"]]["deps"]
                    if packages[client["pkg"]]["name"] == "reqwest" and any(
                        kind["kind"] in (None, "normal") for kind in client["dep_kinds"]
                    )
                )
    normal = set()
    queue = list(metadata["workspace_members"])
    while queue:
        key = queue.pop()
        if key in normal:
            continue
        normal.add(key)
        for dependency in nodes[key]["deps"]:
            if packages[key]["name"] == "transmog-desktop" and (
                packages[dependency["pkg"]]["name"] == "tauri-plugin-updater"
                or dependency["pkg"] in updater_clients.get(key, set())
            ):
                continue
            if any(kind["kind"] in (None, "normal") for kind in dependency["dep_kinds"]):
                queue.append(dependency["pkg"])
    selected = [packages[key] for key in normal]
    forbidden = sorted({p["name"] for p in selected} & FORBIDDEN)
    if forbidden:
        raise RuntimeError(f"Forbidden production TLS crates on {target}: {', '.join(forbidden)}")
    for name in ("boring", "boring-sys"):
        # Cargo package IDs distinguish registry, Git revisions and local paths,
        # even when separate TLS stacks report the same version.
        identities = {p["id"] for p in selected if p["name"] == name}
        if len(identities) != 1:
            raise RuntimeError(f"Expected one {name} package on {target}; found {sorted(identities)}")


def main():
    for target in TARGETS:
        metadata = json.loads(command(["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target], capture=True).stdout)
        check_graph(metadata, target)
        print(f"{target}: one BoringSSL family; desktop updater is the only isolated HTTPS-client exception.")


if __name__ == "__main__":
    main_guard(main)
