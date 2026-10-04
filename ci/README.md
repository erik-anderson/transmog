# Deferred hosted automation

Hosted CI is intentionally not enabled yet. The prepared GitHub Actions
definitions live under `ci/github-actions/` rather than the recognized
`.github/workflows/` directory, so pushing this repository cannot trigger them
through `push`, `pull_request`, `schedule`, or `workflow_dispatch` events.

Local validation remains authoritative while hosted automation is deferred.
Use the commands documented in `docs/testing.md`, including the read-only Linux
Docker/WSL2 matrix, before publishing changes.

Enabling hosted CI is an explicit future repository decision. When a human
decides it is appropriate:

1. Review action versions, permissions, runner images, secrets, retention, and
   cost controls against the repository's current policy.
2. Move the selected definitions into `.github/workflows/`.
3. Revisit their event triggers before pushing the enabling commit; the parked
   files preserve the originally proposed triggers as design material, not as
   approved automation policy.
4. Observe the first Windows, Linux, macOS, fuzz, browser, and performance runs
   and record any platform-specific findings in the applicable plan.
