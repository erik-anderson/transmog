# Hosted Windows packaging and releases

## Signed draft releases

Dispatch **Windows signed release** (`.github/workflows/windows-release.yml`)
on `main` for Canaries, or a major/minor release branch such as `release/0.1`
for Beta/Stable. It uses standard
`windows-2025` runners and skips private repositories
and other branches. It has no automatic push, tag, schedule, or pull-request
trigger. Runs are serialized, time-limited, and retain intermediate artifacts
for seven days; compiled targets and signing tools are never restored from caches.

For each release, set and commit the version with
`pwsh ./scripts/set-release-version.ps1 -Version <major.minor.patch>`,
push it to the selected branch, then use
**Actions > Windows signed release > Run workflow** and select that branch.
**Branch default** uses the checked-in Canary/Beta/Stable track. A release branch
can override its build to Beta or Stable; main can only build Canary.
Approve the signing environment after the build passes. Download and review the
resulting draft's installer, then edit and publish that same draft when ready.
The [manual release process](../docs/windows-release.md#manual-github-release-process)
documents version selection, rebuilds, previews, and publication. The workflow
checks that Cargo, Tauri, and desktop UI versions agree before compiling.
For automation, prefer GitHub CLI or REST calls. If `gh` is missing, reuse the
existing Git credential helper with the
[PowerShell API fallback](../docs/windows-release.md#manual-github-release-process)
before trying browser automation. The fallback preserves the source-commit check
and the protected signing approval.
The canonical semantic version, source channel, and release track live in `release-version.json`;
Cargo/npm/Tauri, release tags, installer names, and product diagnostics use that same semantic version.
Published versions, wrong branch lines/tracks, and reserved Canary lines fail
before build setup. **Maintain release and Canary versions** initializes new
release branches to Beta and reserves their line at branch creation by advancing
main to the next minor Canary version. Existing later lines are preserved, and
major increments require human action. Publication advances only the publishing
branch's patch version; Beta promotion persists Stable without another increment.
Release-branch publication leaves main unchanged. The job has only `contents: write`, with no
Azure identity or signing access. Release branches use neutral Release product
branding, so reviewed Beta assets can become Stable without another build/signing.

| Job | GitHub permissions | Azure access |
| --- | --- | --- |
| Build and tests | `contents: read` | None |
| Protected signing | `contents: read`, `id-token: write` | Sign only the selected production profile |
| Installer smoke test | `contents: read` | None |
| Negative identity test | `contents: read`, `id-token: write` | Authentication must be rejected |
| Provenance attestation | `contents: read`, `id-token: write`, `attestations: write` | None |
| Draft publication | `contents: write` | None |

The build runs the full locked repository gate, browser workspace
tests, and 3-minute WebView soak before credentials exist. Soak progress logs
include UTC timestamps and elapsed/remaining time once per
minute, with a completion timestamp. Same-run immutable
artifacts carry hashes, commit, run ID, version, and target architecture. The
build also validates matching full PDBs for all four released executables and
retains them in a versioned public symbols ZIP. The archive includes the source
commit and debug identifiers and receives release checksums and provenance.
The signing job checks that catalog before restoring the app, two bundled helpers,
standalone CLI and UI assets;
it bundles those outputs without recompiling the application or running npm
installation scripts with Azure credentials.

Producer jobs pass immutable artifact IDs to downstream jobs, so retrying a
failed job can retain the successful build and soak from an earlier attempt.
Signing retries timestamp-service failures up to three times with short delays;
permission errors and other failures stop immediately. Every attempt starts
from the original input bytes and still requires valid signatures and timestamps.

Browser workspace checks run first so UI failures do not wait for Rust compilation.
The release builder first fetches the locked Cargo dependencies, so offline UI
Credits generation can read their metadata and license files on a fresh runner.
The repository-wide Clippy/tests then run before optimized compilation. That full gate
replaces the packaging wrapper's narrower test pass. Tauri forwards all four
package selections to one Cargo release build, and sidecars are staged
later during bundling. Clippy/development and release artifacts remain separate;
this avoids redundant test/package passes without removing the quality checks.
The CLI is signed and published separately, with version, checksum, signature
and provenance checks. The installer qualification gate confirms it is excluded
from the app install directory.

The hosted WebView check isolates product data and temporarily sets loopback
DevTools arguments for the desktop executable through machine policy. WebView2
150+ ignores environment overrides in elevated hosts; standard hosted Windows
jobs run elevated. The wrapper restores the prior per-app policy afterward and
refuses that mode outside disposable GitHub-hosted runners. The prerequisite
setup verifies an Evergreen runtime is present, installing Microsoft's signed
bootstrapper only on those runners when required. Runtime payloads remain absent
from the Transmog installer. Failure artifacts contain startup logs/reports only,
never the isolated profile or its product data.

For runner diagnostics, **Windows hosted WebView startup probe** reuses the
hash-verified unsigned baseline solely as a test application. Its optional fast
smoke mode avoids recompiling Rust. This baseline is never used for a signed
release; release artifacts still require same-run source/hash validation.

The `release-signing` environment allows `main` and `release/*`. It requires
`erik-anderson` approval, allows self-review for this solo repository, and disables
administrator bypass. Approve the pending signing job after reviewing its source
commit and successful build. Azure uses a dedicated, secretless Entra application
with one federated identity for this exact repository/environment subject and one
Certificate Profile Signer assignment on the production certificate profile.
It has no subscription/resource-group/account-wide roles or application API
permissions. The login uses the tenant without requiring subscription discovery.

Protected environment secrets mask values from the start of each job:

- `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, and `AZURE_SUBSCRIPTION_ID`;
- `SIGNING_ENDPOINT`, `SIGNING_ACCOUNT_NAME`, `SIGNING_CERTIFICATE_PROFILE`;
- `SIGNING_DENIED_PROFILE`.

Updater signing uses `TRANSMOG_UPDATER_PRIVATE_KEY` and the optional
`TRANSMOG_UPDATER_PRIVATE_KEY_PASSWORD` in that same environment. This key is
separate from Azure's Authenticode identity and must match the desktop's embedded
public key. Keep a secure backup of the ignored local private key.

`SIGNING_PUBLISHER` is an environment variable because the Authenticode publisher
is public. Azure authentication remains secretless OIDC; storing identifiers in
GitHub's secrets facility is a log-privacy measure, not a client-secret login.

The negative identity job uses repository secrets `AZURE_SIGNING_PROBE_CLIENT_ID`
and `AZURE_SIGNING_PROBE_TENANT_ID` for the same application/tenant IDs. These are
configuration identifiers, not passwords or signing credentials; storing them as
secrets masks logs and avoids carrying operational configuration in public
artifacts. Signing logs mask account, endpoint, and profile configuration, and
release reports omit those values. The Authenticode publisher remains public.

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
verifies the Sigstore bundle against this repository, workflow, the selected branch, and the
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
For a hotfix, replace `refs/heads/main` with its source branch.

Publication creates or updates only a draft for the configured product
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

The workflow selects Node and uses the Rust version from `rust-toolchain.toml`.
[`setup-windows-ci.ps1`](../scripts/setup-windows-ci.ps1) owns LLVM, CMake, Ninja
and NASM download pins and verifies SHA-256 before extraction. Visual Studio C++
tools and the Windows SDK come from the runner image. The checked-in workflow,
dependency lockfiles and separately hash-pinned V8 archive own exact versions.

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
