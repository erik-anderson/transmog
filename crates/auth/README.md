# Native authentication inspection

This crate decodes authentication evidence without sending traffic or verifying
credentials. It runs in the native process. The desktop adapter supplies retained
input and authorizes value access; the UI renders masked findings.

## Reading the source

1. [lib.rs](src/lib.rs) defines inputs, findings, source ranges and the public
   `inspect` and `inspect_as` entry points.
2. [extract.rs](src/extract.rs) selects header, URL and body candidates.
   [body.rs](src/body.rs) handles character decoding and inert form, JSON, XML
   and HTML extraction.
3. [http.rs](src/http.rs), [jose.rs](src/jose.rs) and [saml.rs](src/saml.rs) decode
   their protocol envelopes. [windows.rs](src/windows.rs) dispatches to separate
   [NTLM](src/windows/ntlm.rs), [ASN.1](src/windows/asn1.rs),
   [Kerberos](src/windows/kerberos.rs) and [SPNEGO](src/windows/spnego.rs) modules.
4. [context.rs](src/context.rs) owns one inspection's private values, output
   limits and cancellation signal. [limits.rs](src/limits.rs) names the shared
   size and structure safeguards.
5. [document.rs](src/document.rs) and [scratch.rs](src/scratch.rs) produce explicit
   converter output from the same decoders. [catalog.rs](src/catalog.rs) defines
   the stable conversion IDs and format hints.

The [desktop service](../app/src/auth.rs) binds findings to the active pane,
evidence revision and window. Reveal, copy, source navigation and converter
handoff reauthorize that binding. See [Authentication inspection](../../docs/auth-inspection.md)
for the user workflow and interpretation limits.

## Adding a format or field

Keep protocol layouts and discriminators beside their decoder. NTLM's
`MessageLayout` groups fixed offsets by message type; ASN.1 uses named tags.
Use checked slices for wire lengths and preserve original source coordinates
through decoding transformations.

Add readable values through `Context::field`, choosing sensitivity explicitly.
Use `technical_field` for raw representations and secondary metadata.
Full values and typed converter material are excluded from ordinary inspection
serialization. Reserve bytes before retaining additional decoder-owned buffers;
`field` reserves its own value storage.

Report malformed, unsupported or incomplete evidence through `Finding::issue`.
Decoding does not establish trust. Keep diagnostics free of captured secrets and
preserve duplicate fields and exact number spellings.

Add meaningful fixtures with values and ranges, malformed lengths, cancellation
and recovery. Use the existing protocol tests and fuzz target for additional
boundary coverage.

## Safeguards and cancellation

Input, decoded data, retained values, collections, fields and nesting have finite
limits. The aggregate byte allowance remains necessary because this parser runs
inside the native process. Cancellation checks are independent of those limits;
there is no work-step counter. Poll `Context::is_canceled` in scans and recursive
decoders so leaving an inspection can stop pending work.

## Verification

From the repository root:

```text
python scripts/dev-env.py --check
python scripts/dev-env.py --run cargo fmt -p transmog-auth -- --check
python scripts/dev-env.py --run cargo clippy --locked -p transmog-auth --all-targets -- -D warnings
python scripts/dev-env.py --run cargo test --locked -p transmog-auth
python scripts/dev-env.py --run cargo test --locked -p transmog-app auth::
```

Follow the [unused-dependency audit](../../docs/building.md#unused-dependency-audits)
when changing Rust code or dependencies. The [auth workflow checks](../../docs/testing.md#desktop-and-capture-workflows)
cover browser presentation and native WebView2 acceptance.
