# Hosted workflows

Active definitions live in [`.github/workflows`](../.github/workflows/):

| Workflow | Trigger and purpose |
| --- | --- |
| [Windows signed release](../.github/workflows/windows-release.yml) | Manual dispatch on `main` (Canary) or `release/<major>.<minor>` (Beta/Stable); qualifies x64 and ARM64 and creates a signed draft. |
| [Prepare reusable Windows V8](../.github/workflows/prepare-windows-v8.yml) | Prepares the engine on changes to its inputs on main, by manual dispatch, or for a calling workflow. |
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
protected x64 signer. Web UI and browser acceptance run once on native x64,
with one immutable source/run-bound artifact shared by both architectures.
Each architecture must still pass its native repository, certificate, archive,
optimized CLI, symbols and three-minute WebView soak gates before signing.
Policy checks share an immutable advisory snapshot during builds and
refresh advisories before signing. Python requires 3.11+; `PYTHONUTF8=1` keeps
subprocess output consistent with the tooling's UTF-8 decoder.

| Job | GitHub permissions | Azure access |
| --- | --- | --- |
| Policy, build and tests | `contents: read` | None |
| Reusable V8 preparation and native verification | `contents: read`, `actions: read` | None |
| Protected signing | `contents: read`, `id-token: write` | Sign only the selected production profile |
| Installer smoke test | `contents: read` | None |
| Negative identity test | `contents: read`, `id-token: write` | Authentication must be rejected |
| Provenance attestation | `contents: read`, `id-token: write`, `attestations: write` | None |
| Draft publication | `contents: write` | None |

CI caches dependency downloads, native tools and prepared V8 builds. It verifies
the restored native and V8 archives by hash. Application binaries and signed
outputs pass between jobs as artifacts. Immutable artifact IDs bind each
transfer to its hashes, commit, run ID, version and architecture.
Hosted builds install the [pinned MSVC toolset](../scripts/setup-windows-msvc.ps1)
for both architectures and use the Windows SDK on the runner. Reusing a
prepared V8 build requires matching engine inputs, target ABI and toolchain. The workflow checks cross-built ARM64 archives on a native consumer
or verifies a matching prior native check. Disabling the cache forces fresh
preparation and verification. See the [preparation workflow](../.github/workflows/prepare-windows-v8.yml)
for matching rules, deadlines and failure reports.

The signer verifies the release artifacts before bundling; it neither recompiles the application
nor runs npm installation scripts with Azure credentials. See
[release symbols](../docs/windows-release.md#crash-investigation-symbols) for PDB
validation and the public symbols archives.

Retrying failed jobs can reuse successful upstream artifacts. Timestamp-service
failures retry up to three times from the original input bytes; permission and
other signing failures stop immediately. Every attempt still requires valid
signatures and timestamps. The CLI is signed separately, included beside the
desktop for MCP connections, and published as a standalone asset. Lifecycle
checks run before signing and against the final CLI.

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

Run the [Windows unsigned installer workflow](../.github/workflows/windows-installer.yml)
manually to build an x64 development installer in the public repository. It
retains the installer, `SHA256SUMS` and build measurements for seven days.

## Run an unsigned build

In **Actions → Windows unsigned installer → Run workflow**, select the revision.
Leave **Reuse dependency downloads** off for a cold build. The V8 preparation
job has read access to repository contents and Actions. The packaging job has
read access to contents and verifies the engine artifact from that run before
building on `windows-2025`.

The [CI packaging script](../scripts/ci-windows-installer.ps1) installs the
locked root and UI dependencies, runs the packaging checks and verifies the
installer is unsigned. **windows-build-measurements** records phase times,
tool versions, disk usage, installer size and hash, and any reported failure.
Its duration starts with the packaging script, after V8 preparation and runner
setup.

## Compare a cached build

Enable **Reuse dependency downloads** and compare builds of the same revision.
A run can reuse matching native-tool downloads, Cargo downloads and prepared V8
from an earlier workflow. Check the job summaries for cache hits before
comparing times. The [shared Windows setup](../.github/actions/windows-release-setup/action.yml)
defines the download caches and disables npm caching.

## Additional hosted coverage

Use [the local test gates](../docs/testing.md) for coverage beyond the active
workflows above. Adding hosted platform, browser, fuzz or performance jobs
requires an explicit repository decision and review of triggers, permissions,
runner images, secrets, retention and costs. Base new jobs on the current
runners and local commands, then qualify their first hosted runs.
