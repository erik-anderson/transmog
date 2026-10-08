# Windows release and qualification

Windows is the first supported Transmog desktop target. A release is a signed,
current-user NSIS installer that uses the Evergreen WebView2 runtime already
present on the supported Windows host. The app does not bundle a browser or
WebView runtime, require administrator installation, or fetch runtime UI assets.

## Manual GitHub release process

The normal release path is **commit a version → manually build a signed draft →
review the installer → publish that draft**. Building and publication are separate
decisions. No push or tag automatically starts a release build.

1. Choose an unused `major.minor.patch.revision` version, for example `0.1.0.1`, and run from
   the repository root:

   ```powershell
   pwsh ./scripts/set-release-version.ps1 -Version 0.1.0.1
   git diff
   ```

   `release-version.json` records the four-part version, source channel, and default
   release track. `main` uses `Canary`; a release branch has the neutral `Release`
   source channel and a `Beta` or `Stable` track. The command preserves that track
   unless `-ReleaseType` is supplied. It synchronizes the Cargo workspace and exact
   internal dependency pins, both Cargo lockfiles, the Tauri installer/app version,
   and the private desktop UI package/lockfile. It uses the installed Rust tools
   and cached Cargo dependencies, without updating external dependency versions.
   If cached dependencies are missing, restore them using the normal build setup
   before retrying. `pwsh ./scripts/set-release-version.ps1 -Check` checks version
   consistency without changing files. Each numeric component must fit in
   `0..65535`. Cargo, npm, and Tauri require SemVer, so `0.1.0.1` maps internally
   to `0.1.0+1`; release tags, installer filenames/display versions, Windows numeric
   version resources, the product title/diagnostics, and the SBOM use the four-part
   product version. The NSIS template compares all four numeric parts for updates
   and downgrade prevention. The app title and diagnostics add `Canary` on main.
   Beta and Stable installers from a release branch use neutral product branding,
   so the same reviewed Beta installer can be promoted to Stable without rebuilding.

2. Review the version diff, commit the version changes with the code to release,
   and push them to `main` (or the approved hotfix branch described below). This
   commit is the release source. The workflow checks
   committed versions before compilation; the release manifest, installer, and
   SBOM use that version. Changing the draft's title or tag cannot change the
   version embedded in an already-built installer. Published versions, mismatched
   branch majors/tracks, and Canaries on a reserved release major fail before builds.

3. Open [Actions → Windows signed release](https://github.com/erik-anderson/transmog/actions/workflows/windows-release.yml),
   click **Run workflow**, select **main** (or the approved hotfix branch), and
   click **Run workflow** again. With
   Leave **Release type** at **Branch default** to use the checked-in track. You
   can explicitly choose **Beta** or **Stable** on a release branch; main permits
   only **Canary**. With GitHub CLI installed and authenticated, the equivalent is:

   ```powershell
   gh workflow run windows-release.yml --repo erik-anderson/transmog --ref main
   ```

   The workflow builds the selected branch's commit at dispatch. Later pushes to
   that branch do not change the run. The first successful build took about an hour,
   including a 30-minute WebView soak; signing and qualification then took a few
   minutes after approval. The separate **Windows unsigned installer** workflow
   is available for development builds that do not need signing or a release.

4. When the build passes, open the run's **Review deployments** prompt and approve
   `release-signing` after checking the source commit. This authorizes signing;
   it does not publish a release. Wait for all six jobs to succeed, then follow
   the signed-draft link in the final job summary or open
   [Releases](https://github.com/erik-anderson/transmog/releases).

5. Download the installer from the `v<version>` draft, confirm the desired version
   and publisher, and exercise the app features you intend to ship. Review the
   attached checksums, test reports, and provenance as needed. The clean Windows
   11 checklist below remains deferred; it is not an additional pipeline approval
   gate. If code needs fixing, commit it and dispatch a new build for the same
   unpublished version. A successful replacement run updates that version's
   draft and assets; repeat your review using the replacement installer. Do not
   edit release notes until the final candidate, since rebuilding resets them.

6. Edit the **existing draft**, add release notes, and choose **This is a
   pre-release** for a Beta or Canary, or clear it for Stable. The workflow sets
   that flag from the selected release type. For Stable, choose **Set as latest release**
   if appropriate. Keep the generated tag/version and the pinned source commit;
   do not retarget it to newer branch code. Click **Publish release** when ready.
   This publishes the same signed installer you reviewed, without rebuilding or
   another Azure signing request. GitHub creates the version tag at the pinned
   commit if it does not already exist; there is no need to create a tag first.

Once published, treat a version as final: the workflow refuses to overwrite it.
Use a new revision or another unused four-part version for subsequent fixes.
Before publication, start a fresh
manual run or use **Re-run all jobs**; **Re-run failed jobs** alone is insufficient
because this workflow's artifacts are specific to the run attempt. Leave the
current run's draft unpublished if it is not the candidate you want to ship.

### Branch creation, Beta-to-Stable promotion, and Canaries

Use one release branch per major: `release/0` for `0.x`, `release/1` for `1.x`,
and so on. Create it when preparing the first Beta of that major. Its version's
major must match its name. For the current `0.x` line, after pushing this tooling:

```powershell
git switch main
git pull --ff-only
git switch -c release/0
git push -u origin release/0
```

Wait for **Maintain release and Canary versions** to finish, then pull the bot's
initialization commit on your release branch. The creation event sets its default
track to Beta with neutral Release branding and moves main to the next full major
Canary version. The same major is checked again at publication as a recovery path.

| Event | Release branch | Main |
| --- | --- | --- |
| Create `release/1` while main is on major 1 or earlier | Existing version, Beta track | `2.0.0.0 Canary` |
| Publish Beta `1.2.3.7` from `release/1` | `1.2.3.8`, still Beta | Later major retained |
| Promote that published Beta to Stable | `1.2.3.8`, switch to Stable | Later major retained |
| Publish Stable `1.2.3.8` from `release/1` | `1.2.3.9`, still Stable | Later major retained |
| Publish Canary `2.0.0.0` from main | Unchanged | `2.0.0.1 Canary` |

The local fixture in `scripts/test-release-lifecycle.ps1` exercises these events
against a disposable Git remote, including retries, old-major hotfixes, and deleted
branches. It is part of the release safety gate and does not contact GitHub or Azure.

Main's reservation rule compares **major numbers**: a release branch with an equal
or higher major moves main to `release-major + 1`, resetting its other components
to zero. Main already on a later major is left alone. Canary publication increments
the least significant component. Canaries are rejected before building and again
before draft creation if `release/<their-major>` exists, covering the window while
the branch-creation hook is pending. Never rewind main to an already-reserved major.

Promote a reviewed Beta by editing its existing published GitHub Release, clearing
**This is a pre-release**, removing Beta from the title, and saving it. Keep the
tag, source commit, and assets. The hook changes that branch's default track to
Stable without consuming another version; subsequent **Branch default** builds
remain Stable. Its attested manifest retains the original Beta build type as
provenance. Alternatively, choose Stable for a new draft on that same branch.
Canaries stay prereleases on main; stable releases come from release branches.

For an urgent fix, use the existing per-major release branch, commit the fix at its
next unused version, build/review the draft, and publish it. Older-major releases
do not bump newer main development. Carry the bug fix back to main separately;
do not merge an old release branch's version metadata over main's newer version.

The signing environment allows `main` and `release/*` with the existing required
review and disabled administrator bypass. Workflow checks narrow that pattern to
the matching numeric major. Azure's environment identity and profile-scoped signer
role need no additional permissions.

Repeated creation/publication events do not increment twice. Promotion persists
Stable, and a delayed Beta event cannot reset that track. Deleted branches are
skipped without being recreated. A reset release branch older than its published
version fails for inspection. Concurrent pushes are retried without force-pushing;
published tags are preserved. Numeric component overflow carries to the next
component; a release branch cannot cross into another major.

The maintenance job has only `contents: write` and no signing environment or
Azure access. It loads its implementation from `main`, then inspects the affected
branch. If branch protection prevents the bot commit, the job fails visibly;
apply the version command manually and commit it through the branch's normal
review process. The published-version preflight still prevents duplicate builds
while the hook is pending or failed. Legacy three-part releases do not trigger
a version bump. Create branches and publish/promote releases through the GitHub
UI or an authenticated user CLI: events created by another workflow's `GITHUB_TOKEN`
do not trigger this hook automatically. These branches must include the release
tooling; start from updated main or a release tag that already contains it.

GitHub documents [manual workflow dispatch](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)
and [editing and publishing releases](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository),
[release events](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#release),
and [workflow-token event behavior](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow#triggering-a-workflow-from-a-workflow).

## Release prerequisites

In addition to [building.md](building.md), the release workstation needs:

- a code-signing certificate available to the current user and usable by the
  Windows SDK `signtool.exe`;
- its 40-hex SHA-1 certificate thumbprint (Windows' signing-certificate
  selector, not a traffic-certificate identity);
- access to the configured timestamp service; and
- the Tauri NSIS artifacts in cache when the final build must run without
  network access.

No certificate identity, private key, token, or password belongs in this
repository. The release wrapper injects the certificate thumbprint in a
temporary ignored config overlay and removes that overlay after packaging.

For hosted releases, dispatch **Windows signed release** on an approved branch and
approve its protected `release-signing` job after the build passes. The workflow
uses Azure Artifact Signing through profile-scoped OIDC, signs the inner binaries
and NSIS bundle, verifies timestamps/publisher, tests permission rejection, and
creates a GitHub provenance attestation for the final signed installer. It creates
only a draft release. Its setup, permissions, artifacts, and verification command
are described in [`ci/README.md`](../ci/README.md). The separate unsigned installer
workflow remains available for development.

For the alternative local-certificate path, create a signed installer:

```powershell
pwsh ./scripts/package-windows.ps1 `
  -SigningCertificateThumbprint 0123456789ABCDEF0123456789ABCDEF01234567
```

The wrapper reproduces UI assets, runs the desktop/application/host test gate,
builds the NSIS bundle, requires a valid Authenticode result, and prints the
installer's SHA-256. For local installer development only:

```powershell
pwsh ./scripts/package-windows.ps1 -UnsignedDevelopment
```

Repeat the package command with Cargo offline and network blocked before
release to prove the cached build path. The packaging wrapper fails if the
Tauri configuration changes away from `skip`; this prevents accidentally
shipping a fixed runtime, bootstrapper, or offline WebView installer.

```powershell
pwsh ./scripts/package-windows.ps1 `
  -SigningCertificateThumbprint 0123456789ABCDEF0123456789ABCDEF01234567 `
  -Offline
```

## Installer and lifecycle policy

- Installation is per-current-user. Downgrades are blocked.
- Evergreen WebView2 is an operating-system prerequisite and is never bundled.
  Diagnostics report the actual runtime version.
- Tauri's single-instance plugin is registered first. A second launch focuses
  the existing main window and exits; it cannot create a competing proxy owner.
- **Prepare safe update handoff** detaches breakpoint control, seals/stops
  active capture, stops the proxy, and restores host state. The app must then
  be closed before the installer runs.
- The NSIS pre-uninstall hook refuses to continue while the app is running. In
  update mode it invokes `--prepare-update`; in removal mode it invokes
  `--uninstall-cleanup`. A nonzero maintenance result aborts removal so an
  unresolved system-proxy or trust-store state is never hidden by deleting the
  executable.
- Maintenance mode restores only an existing Transmog proxy journal. Uninstall
  removes only the exact SHA-256 certificate in
  `certificate-ownership-v1.json`; a similarly named or otherwise unrecorded
  root is never touched.

## Owned data

The installer owns its executable, shortcuts, and uninstall registration.
Runtime product state under `%LOCALAPPDATA%\Transmog` owns these names:

- `proxy-recovery-v1.json`;
- `certificate-ownership-v1.json`;
- `preferences.<generation>.json` and quarantined variants;
- `.transmog-state-*.tmp`; and
- `diagnostics.jsonl` plus its single rotation.

Uninstall removes only those regular files. Unknown files and directories are
left in place. User-selected CAs, private keys, captures, JSONL files, SAZ
files, and support bundles are user artifacts and are never deleted by the
uninstaller. Updates preserve product state and certificate ownership.

## Desktop qualification gates

Run the release WebView gate (the default fast soak uses 100 bounded refreshes):

```powershell
pwsh ./scripts/test-windows-desktop.ps1
```

For release qualification, use at least 30 minutes:

```powershell
pwsh ./scripts/test-windows-desktop.ps1 -SoakMinutes 30
```

The gate uses the packaged WebView technology and verifies startup, CSP and
browser-error absence, semantic landmarks and accessible names, the Chromium
accessibility tree, keyboard-focusable native controls, forced-colors behavior,
reduced motion, 200% device scale, long localized labels, bounded JavaScript
heap, bounded process working-set growth, and second-launch handoff. Tauri and
NSIS declare per-monitor-v2 DPI awareness.

Also run the complete repository gate and standalone interoperability matrix:

```powershell
pwsh ./scripts/test.ps1
pwsh ./scripts/test-interop.ps1
```

The interoperability matrix proves the proxy observed the exchange and covers
standalone curl/Chromium with Nginx, Apache, and Caddy across supported HTTP and
content-coding paths. Certificate validation remains enabled.

## Clean-machine release checklist

The maintainer has deferred this checklist for the initial pipeline integration.
Hosted Windows Server installer and WebView checks do not establish clean Windows
11 qualification. Draft release evidence records that limitation.

Use a disposable, fully updated Windows 11 VM with no Transmog state:

1. Confirm the supported Windows image has an updated Evergreen WebView2
   runtime, disconnect networking, and install the signed NSIS bundle. Confirm
   a valid Authenticode publisher and that no WebView payload is installed by
   Transmog.
2. Launch Transmog twice. Confirm one process/window and focus handoff.
3. Choose **Set up HTTPS interception**, verify the warning precedes the OS
   prompt, and approve current-user trust. Start the proxy and verify that the
   current-user proxy is applied, live traffic follows automatically, and HTTPS
   certificate validation remains on.
4. Exercise HTTP/1.1, HTTP/2, HTTP/3 egress, gzip, deflate, Brotli, zstd,
   WebSocket inspection, one request edit, one response edit, composer replay,
   native capture, JSONL export, and strict/extended SAZ export. For every case,
   verify the Transmog session/capture evidence so a direct path cannot pass.
5. Exercise keyboard-only navigation, a screen reader, 100%, 150%, and 200%
   scaling, Windows high contrast, reduced motion, and deliberately long text.
6. Start the proxy, terminate Transmog, and relaunch. Confirm startup recovery
   restores the exact previous registry values without requiring a manual
   recovery click. Also verify normal close and an OS-requested exit restore
   before process termination.
7. Run **Prepare safe update handoff**, install a newer signed build offline,
   and confirm preferences and the exact app-owned certificate survive.
8. Uninstall and approve exact certificate removal. Confirm the system proxy is
   not enabled, the ownership record and declared app data are gone, unrelated
   current-user roots remain, and user-created captures/exports remain.

Record OS build, WebView2 version, installer SHA-256, signer, test command
outputs, and the pre/post proxy and certificate inventories in the release
evidence. Certificate install/remove dialogs are intentionally manual; release
automation must wait for the human action rather than bypass validation.
