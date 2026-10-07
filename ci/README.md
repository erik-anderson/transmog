# Hosted Windows packaging and deferred automation

The **Windows unsigned installer** workflow is enabled under
`.github/workflows/windows-installer.yml`. It runs only when a maintainer
manually dispatches it, uses the standard `windows-2025` x64 runner, and grants
only `contents: read`. It skips private repositories to preserve the public
repository's free runner usage. It has no Azure access or signing credentials,
does not publish a GitHub Release, and retains its artifacts for seven days.

## Run the first build

After the workflow is on the default branch, open GitHub **Actions > Windows
unsigned installer > Run workflow** and select the revision to build. Leave
**Reuse dependency downloads** disabled for the first cold build.

The workflow installs Node 24.21.0 and the Rust version from
`rust-toolchain.toml`. `scripts/setup-windows-ci.ps1` downloads LLVM 22.1.4,
CMake 4.4.4, Ninja 1.13.2, and NASM 3.02 from their official release sites and
verifies pinned SHA-256 hashes before extraction. Visual Studio C++ tools and
the Windows 11 SDK 10.0.26100 come from the runner image. Dependency lockfiles
and the separately hash-pinned V8 archive remain authoritative.

`scripts/ci-windows-installer.ps1` installs UI dependencies with `npm ci` and
calls the existing unsigned development packaging gate. That gate checks the
UI, runs locked application/host/desktop tests, builds the release binaries,
and packages NSIS. The CI wrapper requires an unsigned result and uploads the
installer plus its `SHA256SUMS` file as a development artifact.

The job summary and **windows-build-measurements** artifact record phase times,
tool versions, free disk before and after the build, Cargo target size, commit,
installer size/hash, and failure information. Build duration excludes checkout
and Node setup; GitHub shows the complete job duration separately. Disk
measurements are snapshots, not peak usage. A failed build still uploads any
measurements produced before termination. The job timeout is 120 minutes and
concurrency is limited to one packaging run.

## Compare a cached build

Once the cold build succeeds, rerun with **Reuse dependency downloads** enabled.
The first enabled run populates the caches; a later enabled run can measure
reuse. Caches contain npm, Cargo source downloads, native tool archives, and the
V8 archive. They do not contain compiled Cargo targets. Every native/V8 archive
is hash-checked after restoration. Keep GitHub's cache limit at its included
allowance; increasing it is not required for this workflow.

The optional caches are for this unsigned workflow. A future signing workflow
must review cache trust and permissions separately.

## Deferred workflows

The broader definitions remain under `ci/github-actions/` rather than
`.github/workflows/`. Pushing the repository does not trigger their proposed
push, pull-request, schedule, or dispatch events.

Local validation remains authoritative while hosted automation is deferred.
Use the commands documented in `docs/testing.md`, including the read-only Linux
Docker/WSL2 matrix, before publishing changes.

Enabling those broader workflows remains an explicit repository decision:

1. Review action versions, permissions, runner images, secrets, retention, and
   cost controls against the repository's current policy.
2. Move the selected definitions into `.github/workflows/`.
3. Revisit their event triggers before pushing the enabling commit; the parked
   files preserve the originally proposed triggers as design material, not as
   approved automation policy.
4. Observe the first Windows, Linux, macOS, fuzz, browser, and performance runs
   and record still-open platform work in `docs/roadmap.md`.
