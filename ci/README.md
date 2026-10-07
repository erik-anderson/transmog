# Hosted Windows packaging and releases

## Signed draft releases

Dispatch **Windows signed draft release** (`.github/workflows/windows-release.yml`)
on `main`. It uses standard `windows-2025` runners and skips private repositories
and other branches. It has no automatic push, tag, schedule, or pull-request
trigger. Runs are serialized, time-limited, and retain intermediate artifacts
for seven days; compiled targets and signing tools are never restored from caches.

| Job | GitHub permissions | Azure access |
| --- | --- | --- |
| Build and tests | `contents: read` | None |
| Protected signing | `contents: read`, `id-token: write` | Sign only the selected production profile |
| Installer smoke test | `contents: read` | None |
| Negative identity test | `contents: read`, `id-token: write` | Authentication must be rejected |
| Provenance attestation | `contents: read`, `id-token: write`, `attestations: write` | None |
| Draft publication | `contents: write` | None |

The build runs the locked packaging tests, full repository gate, browser workspace
tests, and 30-minute WebView soak before credentials exist. Same-run immutable
artifacts carry hashes, commit, run ID, version, and target architecture. The
signing job checks that catalog before restoring the three binaries and UI assets;
it bundles those outputs without recompiling the application or running npm
installation scripts with Azure credentials.

The `release-signing` environment allows only the `main` branch, requires
`erik-anderson` approval, allows self-review for this solo repository, and disables
administrator bypass. Approve the pending signing job after reviewing its source
commit and successful build. Azure uses a dedicated, secretless Entra application
with one federated identity for this exact repository/environment subject and one
Certificate Profile Signer assignment on the production certificate profile.
It has no subscription/resource-group/account-wide roles or application API
permissions. The login uses the tenant without requiring subscription discovery.

Environment variables (identifiers and configuration, not secrets):

- `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, and `AZURE_SUBSCRIPTION_ID`;
- `SIGNING_ENDPOINT`, `SIGNING_ACCOUNT_NAME`, `SIGNING_CERTIFICATE_PROFILE`;
- `SIGNING_PUBLISHER` and `SIGNING_DENIED_PROFILE`.

Keep subscription identifiers, account endpoint, certificate-profile names, and
expected publisher in the protected environment configuration. A separate test
profile has no signer assignment: a signing request to it must fail with HTTP 403.
The identity test must also prove that Azure rejects an OIDC token issued outside
the protected environment. Ordinary build and pull-request contexts have no
trusted Azure identity.

The hash-pinned Artifact Signing client uses only Azure CLI credentials from the
OIDC login. Tauri's custom sign command signs the desktop executable, both helper
executables, copied NSIS plugins, uninstaller, and installer with SHA-256 and
Microsoft RFC 3161 timestamps. Verification requires a valid chain, exact expected
publisher, and timestamp. An Azure-free job then verifies installed executables
and the uninstaller, exercises maintenance/removal, and checks that the smoke test
has not changed the user proxy or root certificates.

`actions/attest` generates provenance for the **final signed installer**, release
manifest, and dependency SBOM file. Its SHA is pinned, and the attestation job
verifies the Sigstore bundle against this repository, workflow, `main`, and the
source commit. This is a provenance attestation, not an assertion that the SBOM
includes every OS/NSIS component. The bundle is included in the draft release.
Consumers can verify the downloaded installer with a recent GitHub CLI:

```powershell
gh attestation verify ./Transmog_0.1.0_x64-setup.exe `
  --repo erik-anderson/transmog `
  --signer-workflow erik-anderson/transmog/.github/workflows/windows-release.yml `
  --source-ref refs/heads/main
```

Add `--bundle ./provenance.sigstore.json` to use the downloaded bundle and
`--source-digest <release-commit>` to enforce a particular source revision.

Publication creates or updates only a draft prerelease for the configured Tauri
version. It refuses to overwrite a published release. The draft contains the
installer, SHA-256 catalog, signing and permission reports, desktop/installer test
evidence, third-party notices, SBOM, source manifest, and attestation bundle. The
clean Windows 11 checklist is deferred by the maintainer; public publication
remains a separate decision. See [`docs/windows-release.md`](../docs/windows-release.md).

## Unsigned development installers

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

The optional caches are for this unsigned workflow. The signed release workflow
uses fresh tools and downloads instead.

## Initial hosted verification

The [first cold run](https://github.com/erik-anderson/transmog/actions/runs/37586689686)
passed on 2026-10-07 at commit `b52c2f8`, with dependency caching disabled.
The full job took 21 minutes 56 seconds; the measured build phases took
1,291.72 seconds. The packaging gate passed 73 tests with one existing ignored
test. The downloaded 23.12 MiB NSIS installer was confirmed unsigned, and its
SHA-256 matched both the uploaded checksum file and build report.

Cargo targets occupied 10.80 GiB after the build. This supports caching source
downloads while keeping compiled targets out of the repository's included
10 GiB cache allowance. Cached-run timing has not yet been measured. Installer
and report downloads are retained for seven days on GitHub; the run history
remains the reference for this baseline.

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
