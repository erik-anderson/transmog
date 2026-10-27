# Hosted workflows

Active definitions live in [`.github/workflows`](../.github/workflows/):

| Workflow | Trigger and purpose |
| --- | --- |
| [Windows signed release](../.github/workflows/windows-release.yml) | Manual dispatch on `main` (Canary) or `release/<major>.<minor>` (Beta/Stable); qualifies x64 and ARM64 and creates a signed draft. |
| [Windows unsigned installer](../.github/workflows/windows-installer.yml) | Manual development packaging on x64; no release publication or signing access. |
| [Windows hosted WebView startup probe](../.github/workflows/windows-desktop-probe.yml) | Manual runner diagnostics using a verified unsigned baseline. |
| [Maintain release and Canary versions](../.github/workflows/advance-release-version.yml) | Branch creation and release events; maintains version tracks and update feeds. |
| [Native SDK](../.github/workflows/sdk.yml) | Relevant pull requests or manual dispatch; Windows/Ubuntu C++, C and .NET checks. |

Broader platform, browser, fuzz and performance coverage remains
[planned](#additional-hosted-coverage). The workflow files own exact triggers, runner
versions, action pins, timeouts and permissions.

## Signed draft releases

Complete [local release qualification](../docs/windows-release.md#local-release-qualification-before-dispatch)
on the clean candidate commit before dispatch. The [manual release process](../docs/windows-release.md#manual-github-release-process)
owns version selection, branch policy, dispatch, signing approval and publication.
Publishing a reviewed draft does not rebuild its assets.

The workflow runs only in public repositories on approved branches. It qualifies
x64 on `windows-2025` and ARM64 on `windows-11-arm`, then bundles both on the
protected x64 signer. Each architecture must pass the repository, browser,
certificate, archive, optimized CLI and three-minute WebView soak gates before
signing. Policy checks share an immutable advisory snapshot during builds and
refresh advisories before signing. Python requires 3.11+; `PYTHONUTF8=1` keeps
subprocess output consistent with the tooling's UTF-8 decoder.

| Job | GitHub permissions | Azure access |
| --- | --- | --- |
| Policy, build and tests | `contents: read` | None |
| Protected signing | `contents: read`, `id-token: write` | Sign only the selected production profile |
| Installer smoke test | `contents: read` | None |
| Negative identity test | `contents: read`, `id-token: write` | Authentication must be rejected |
| Provenance attestation | `contents: read`, `id-token: write`, `attestations: write` | None |
| Draft publication | `contents: write` | None |

Only dependency downloads are cached, keyed by architecture and tool versions.
Native/V8 archives are hash-verified after restoration. Compiled targets, unsigned
payloads and signed outputs are never restored from caches. Same-run immutable
artifact IDs carry hashes, commit, run ID, version and architecture between jobs.
The signer verifies these before bundling; it neither recompiles the application
nor runs npm installation scripts with Azure credentials. See
[release symbols](../docs/windows-release.md#crash-investigation-symbols) for PDB
validation and the public symbols archives.

Retrying failed jobs can reuse successful upstream artifacts. Timestamp-service
failures retry up to three times from the original input bytes; permission and
other signing failures stop immediately. Every attempt still requires valid
signatures and timestamps. The CLI is signed separately and excluded from the
desktop installer; lifecycle checks run before signing and against the final CLI.

Hosted WebView and client-certificate checks isolate product data and temporarily
set loopback DevTools arguments through machine policy for elevated runners.
Wrappers restore prior per-app policy and refuse this mode outside disposable
GitHub-hosted runners. Setup verifies Evergreen WebView2 and installs Microsoft's
signed bootstrapper there only when needed. The product installer contains no
runtime payload. Failure artifacts exclude isolated profiles and product data.

The separate startup probe uses a hash-verified unsigned baseline for diagnostics,
including optional smoke checks without recompilation. That baseline is never a
signed release input.

Configure the `release-signing` environment to allow `main` and `release/*`,
require `erik-anderson` approval, allow self-review for this solo repository, and
disable administrator bypass. Approve the pending signing job after reviewing its source
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

`actions/attest` generates provenance for final installers, standalone CLIs,
symbols, release/update manifests, updater signatures and SBOMs. Its SHA is
pinned, and the job verifies the Sigstore bundle against this repository,
workflow, selected branch and source commit. This is a provenance attestation,
not an assertion that the SBOM
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
evidence, third-party notices, SBOM, source manifest, and attestation bundle.
Record clean-machine qualification or an explicit candidate-specific maintainer
deferral with the release evidence; public publication remains a separate
decision. See [the checklist](../docs/windows-release.md#clean-machine-release-checklist).

## Unsigned development installers

The **Windows unsigned installer** workflow is enabled under
`.github/workflows/windows-installer.yml`. It runs only when a maintainer
manually dispatches it, uses the standard `windows-2025` x64 runner, and grants
only `contents: read`. It skips private repositories to preserve the public
repository's free runner usage. It has no Azure access or signing credentials,
does not publish a GitHub Release, and retains its artifacts for seven days.

## Run an unsigned build

Open GitHub **Actions > Windows
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

The signed workflow has its own architecture-specific dependency-download caches
and independently verifies restored native/V8 archives; neither workflow reuses
compiled targets for releases.

## Additional hosted coverage

Use [the local test gates](../docs/testing.md) for coverage beyond the active
workflows above. Adding hosted platform, browser, fuzz or performance jobs
requires an explicit repository decision and review of triggers, permissions,
runner images, secrets, retention and costs. Base new jobs on the current
runners and local commands, then qualify their first hosted runs.
