# Repository guidance

Python 3.11 or newer is mandatory for local development and agent verification.
Use the portable Python commands in `scripts`; human-facing PowerShell wrappers
are conveniences, while Windows integration checks may remain PowerShell.
Run `python scripts/dev-env.py --check` before building. To run Cargo with the
configured native tools, use `python scripts/dev-env.py --run cargo ...` or
dot-source `scripts/dev-env.ps1` in PowerShell. `TRANSMOG_PYTHON` can select an
explicit interpreter for wrappers. Do not rely on a Windows Store alias that
cannot execute Python.

For TypeScript or `.mjs` changes, install the root tooling with
`npm ci --ignore-scripts` and run `npm run check` from the repository root.
`npm run lint:fix` applies TypeScript ESLint fixes and Prettier formatting across
maintained TypeScript sources and JavaScript modules.

Before dispatching a signed release, complete the
[local release qualification](docs/windows-release.md#local-release-qualification-before-dispatch)
on the candidate commit. All required checks must pass, including formatting,
strict Clippy, workspace tests, browser tests and Windows acceptance checks.
Fix failures and rerun the affected checks before dispatch. Record the tested
commit and verify that the clean, pushed release source matches it.

Before changing desktop behavior visible to a user, follow
[the UX quality routine](docs/ux-quality.md) and read
[the UX design principles](docs/ux-principles.md). This includes UI changes and
changes to commands, defaults, validation, status messages, or asynchronous
behavior that affect the desktop experience.

For desktop UI implementation and verification, also follow
[the scoped UI instructions](apps/desktop/ui/AGENTS.md). Keep durable UX guidance
in the principles document and update it when an intentional product decision
changes an established convention.
