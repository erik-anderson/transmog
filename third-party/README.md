# Bundled license snapshots

These notices cover dependencies whose published Cargo packages omit their
license texts. Each snapshot is pinned to the package version and published
VCS commit in `source.json`. Desktop builds verify both against the locked
dependency graph and fail when an upgrade needs updated notices.

Run `node scripts/collect-license-fallbacks.mjs` to refresh snapshots. Normal
builds use these files offline. Settings → About → Credits displays these notices,
license files from published packages, npm production dependencies and notices
inside vendored native source trees.

After a dependency update, remove snapshots whose package/version is absent
from every checked-in Cargo lockfile. Git history retains notices for old versions.
