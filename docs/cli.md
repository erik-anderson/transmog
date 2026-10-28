# CLI guide

Use the standalone CLI to record a support trace, run a manually configured
proxy, inspect and convert native captures, or connect an MCP agent to the desktop.
Download the signed Windows `transmog-cli.exe` from the project's
[release assets](https://github.com/erik-anderson/transmog/releases). It is
also included in the desktop installer and requires no Rust or
Cargo installation. See the [support capture guide](cli-support-capture.md) for
signature verification and the guided recording flow.

Examples use PowerShell with the release executable in the current folder.
Quote paths containing spaces. When a binary is on `PATH`, use `transmog-cli`
instead; on Linux or macOS, a binary in the current directory is invoked as
`./transmog-cli`. Building the portable CLI from source is covered in
[Build prerequisites](building.md).

## Find a command

```powershell
.\transmog-cli.exe --version
.\transmog-cli.exe help
.\transmog-cli.exe record --help
.\transmog-cli.exe help capture export
```

No arguments prints the command overview. Every command and command group
accepts `--help` or `-h`. Help shows accepted options, defaults, and password
roles before any certificate, proxy, or output-file setup. The
[help source](../crates/proxy-cli/src/help.rs) contains the full option reference.

The [context reader](#read-recorded-context) lists and extracts saved diagnostic
context. Broader case/rule/script CLI ideas in the
[portable investigation design](portable-investigations.md#intended-cli-interface)
are deferred; existing command output and exit behavior remain unchanged.

| Command | Task |
| --- | --- |
| `record` | Guided recording to a new `.tmcap` file; Ctrl+C stops and saves it. |
| `mcp serve` | Run the stdio MCP connector for an agent. |
| `mcp status` / `mcp call` | Check desktop status or call an MCP tool with JSON arguments. |
| `context` | List or extract original recorded configuration, support metadata, incidents and source notes. |
| `roots cleanup` | Restore interrupted Windows proxy settings and remove CLI-owned roots. |
| `serve` | Run a proxy with an existing CA and manually configured clients. |
| `ca generate` | Create a public CA certificate and an OS-user-protected private key. |
| `ca protect` | Protect a matching legacy CA key in place. |
| `ca issue` | Issue a short-lived server certificate and PEM leaf key. |
| `capture inspect` | Summarize and check a native capture, including an unsealed prefix. |
| `capture validate` | Check native integrity and require a complete final seal. |
| `capture seal` | Write an interrupted native capture's complete records to a new sealed file. |
| `capture export` | Convert native records to JSONL, SAZ, or extended SAZ. |

## MCP agent connections

Create a key in **Settings → Integrations** and use the generated agent
configuration; see [MCP setup](mcp.md). Every invocation requires its own
`TRANSMOG_ACCESS_KEY` environment value. Keys are never command-line arguments.
On Windows, preserve the generated `LOCALAPPDATA` environment entry too.
Agents normally launch `mcp serve`; for direct status or tool calls:

```powershell
.\transmog-cli.exe mcp status
'{}' | .\transmog-cli.exe mcp call transmog_status
```

These examples assume the key is already configured in the process environment.
`mcp call` reads a JSON object from stdin. `mcp serve --start-app` lets the agent's
explicit `transmog_connect` call launch the sibling desktop executable; it never
enables MCP or starts collection. Status and direct calls require a running desktop.

## Record a support trace

```powershell
.\transmog-cli.exe record --output .\support-trace.tmcap
```

On Windows, `record` asks to install its public interception root, configures
the current user's system proxy, and restores routing before draining active
requests on Ctrl+C. A second Ctrl+C terminates unfinished work and saves
available evidence. Other platforms require manual client proxy configuration.
`--no-system-proxy` makes Windows routing manual too. `--no-install-root` leaves
certificate setup to you; unattended runs must choose it or `--install-root`.
OS consent still applies to installation and removal.

The initial privacy defaults retain sensitive headers and up to 25,000,000
bytes of each request body. Response bodies and measured byte counts are
recorded. `--redact` and `--request-body-limit` change preferences for later CLI
recordings; `--retain-sensitive` and `--unlimited-request-bodies` change them
back or remove the request cap. Forwarding continues beyond a retained prefix.
URLs, bodies, and metadata may contain private information even with redaction.

Ordinary recording streams to disk. Add `--encrypt` for password protection or
`--circular-buffer auto` to save only the newest retained exchanges at stop.
Memory circular buffers are lost if the process is killed. A failed stop-time
save preserves available evidence at the recovery path printed in the error.
The [support guide](cli-support-capture.md) explains passwords, buffer sizing,
persistence, and cleanup in detail.

## Run a manually configured proxy

Generate a CA with new destination paths:

```powershell
.\transmog-cli.exe ca generate --cert .\transmog-ca.pem --key .\transmog-ca.key
```

The command prints `CA_SHA256` for identifying the public certificate. Install
only `transmog-ca.pem` in the trust store used by the affected client. On
Windows, the current-user certificate manager (`certmgr.msc`) provides an
import wizard under **Trusted Root Certification Authorities → Certificates**.
Some applications use a separate store. Keep the private key under the OS user
that created it; the protected key file is not a PEM input for other tools.
See the [certificate model](certificate-model.md) for platform key protection.

Start the proxy, then configure the client to use `127.0.0.1:8080` for HTTP and
HTTPS:

```powershell
.\transmog-cli.exe serve --ca-cert .\transmog-ca.pem --ca-key .\transmog-ca.key --listen 127.0.0.1:8080
```

Without `--listen`, the listener uses `127.0.0.1:0` and prints the chosen port
as `LISTEN_ADDR`. Both `serve` and `record` default to upstream route `auto`;
`--route h1`, `h2`, or `h3` selects an explicit upstream protocol.
`--upstream-ca-cert FILE` adds PEM roots for private origins to the normal
upstream trust snapshot. It does not install those roots in the OS.
Non-loopback listeners and remote clients require `--allow-remote`.

To record with `serve`, add `--capture .\session.tmcap`; add
`--capture-bodies` to retain body bytes as well as metadata. Sensitive headers
are redacted, and request retention is capped at 25,000,000 bytes per request.
`--capture-bodies` requires `--capture`. Use `record` when you need encryption,
circular retention, or configurable recording privacy. Ctrl+C stops `serve`
and seals a successful capture.
Recording waits for capacity in its bounded event queue. Slow storage slows
requests instead of dropping capture evidence after a time deadline. Actual
writer failures remain errors; use `capture inspect` to check the available
artifact, including after a load test.

Afterward, restore manual client proxy settings and remove the public CA from
each store where you installed it, checking the certificate's exact identity.
`roots cleanup` manages roots owned by `record`; it does not own certificates
created with `ca generate`.

### Verify interception with a proof ID

```powershell
.\transmog-cli.exe serve --ca-cert .\transmog-ca.pem --ca-key .\transmog-ca.key --listen 127.0.0.1:8080 --proof-id local-check-42
```

`--proof-id` makes observable traffic changes so a test can confirm the request
passed through this proxy. It sets `x-intercept-test: local-check-42` and
`Accept-Encoding: identity` on requests. Responses identified by a
`Content-Type` containing `text/html` receive
`x-intercepted-by: local-check-42` and this marker in the HTML:

```html
<meta name="intercept-proxy-proof" content="local-check-42">
```

The marker is inserted before `</head>`, otherwise before `<body`, otherwise
at the start of the body. Its attribute value is HTML-escaped. Non-HTML
responses receive no response marker. HTML processing buffers the decoded
body with a 16 MiB limit and can change response timing. IDs must be valid HTTP
header values without control characters; invalid values fail before startup.
This option belongs to `serve` and is a diagnostic marker, not a capture ID or
authentication credential.

### Protect a legacy CA or issue a test server certificate

```powershell
.\transmog-cli.exe ca protect --ca-cert .\transmog-ca.pem --ca-key .\transmog-ca.key
.\transmog-cli.exe ca issue --ca-cert .\transmog-ca.pem --ca-key .\transmog-ca.key --identity test.example --cert .\leaf.pem --key .\leaf.key --days 7
```

`ca protect` validates that the key matches the certificate before replacing
the legacy key with protected material. `serve` and `ca issue` also migrate
legacy CA keys on load. The public identity and existing trust remain intact.
`ca issue` accepts a DNS name or IP address, defaults to seven days, and allows
one to thirty days. Its leaf key is unencrypted PEM for the test server;
protect that file. These commands do not install certificate trust.

## Inspect, validate, and recover a native capture

```powershell
.\transmog-cli.exe capture inspect --input .\support-trace.tmcap
.\transmog-cli.exe capture validate --input .\support-trace.tmcap
```

`inspect` prints record and exchange counts, loss markers, retained body bytes,
`SEALED`, `TRUNCATED_TAIL`, and the valid prefix length in `VALID_BYTES`.
Inspection and validation check every retained body payload one frame at a
time, without retaining all body bytes. Inspection still keeps exchange IDs
to count unique exchanges. These are native TMCap commands; open SAZ, HAR, or
NetLog through the desktop's import flow.

`validate` prints `VALID=true` only if integrity checks succeed and the file
has a final seal without a truncated tail. This verifies the file's integrity,
not completeness of captured traffic: a valid file can include loss markers,
evicted exchanges, or retained body prefixes.

If inspection reports an unsealed file, write a recovered copy:

```powershell
.\transmog-cli.exe capture seal --input .\support-trace.tmcap --output .\recovered-trace.tmcap
.\transmog-cli.exe capture validate --input .\recovered-trace.tmcap
```

`seal` preserves records through the latest complete checkpoint and the input's
encryption, omits an uncommitted or interrupted tail, and leaves the input untouched. It rejects an already sealed
input. A new destination appears only after the recovered file is sealed
successfully. Missing bytes cannot be reconstructed; retain the original when
investigating corruption or incomplete evidence.

For interrupted `record` runs, restore routing and retry certificate cleanup
with `roots cleanup` before beginning another recording. Use
`roots cleanup --include-persistent` to remove persistent roots too. See
[interrupted-run recovery](cli-support-capture.md#recover-an-interrupted-capture).

## Read recorded context

Use `context` on a native `.tmcap` capture to read information already stored in
it. For example, `record --include-network-context` saves the original machine's
network configuration; profiles and incident marks retain their support context.
The command never runs configuration tools again or changes proxy/certificate
settings. Portable case archives are outside this command's initial scope.

```powershell
.\transmog-cli.exe context --input '.\customer trace.tmcap' --list
.\transmog-cli.exe context --input '.\customer trace.tmcap' --id record/0/networkContext --output '.\network context.json'
```

Without `--id`, JSON output lists available context IDs, kinds, original source
keys/names, recorded collection times when known, and network collection issues.
Use an ID from that input's listing: `record/0/networkContext` is illustrative.
IDs locate fields in this file's metadata; they are not portable case identities
and can change when a capture is rewritten. Unknown collection completeness is
reported as `unknown`, and a null field is `not-collected`. Extracting a null or
missing item fails without creating an output file.

`--id` writes the original JSON value, preserving stored text, Unicode, CR/LF,
timestamps and failure notes. It does not add the receiving computer's state.
`--output` defaults to `-` (stdout); a file destination must be new. File output
is published only after successful extraction. stdout contains only JSON;
prompts, errors and incomplete-prefix notices go to stderr.

Encrypted input asks for a masked password after opening the capture, unless
`--password TEXT` or `--password-file FILE` supplies it. Use the prompt when the
password should stay out of shell history. An incorrect supplied password fails
without asking again. `--no-prompt` disables interactive password requests:

```powershell
.\transmog-cli.exe context --input '.\customer trace.tmcap' --no-prompt
.\transmog-cli.exe context --input '.\customer trace.tmcap' --password 'PASSWORD' --no-prompt --id record/0/networkContext
.\transmog-cli.exe context --input '.\customer trace.tmcap' --password-file '.\protected password.txt' --no-prompt --list
```

A supplied password still works with `--no-prompt`. Without one, protected input
fails immediately instead of waiting for input. Missing passwords also fail when
stdin is not an interactive terminal. Password arguments are accepted only by
`context`; other capture commands retain their existing password-file behavior.

Passwords containing spaces must be quoted. Shell quoting/escaping happens before
Transmog receives the argument; the tool preserves the resulting characters,
including leading/trailing spaces, quotes and backslashes. It does not interpret
backslash escapes a second time. For example, PowerShell 7 supports:

```powershell
.\transmog-cli.exe context --input .\support.tmcap --no-prompt --password 'a password with spaces'
.\transmog-cli.exe context --input .\support.tmcap --no-prompt --password 'spaces, an apostrophe '' and \backslashes\'
.\transmog-cli.exe context --input .\support.tmcap --no-prompt --password "spaces and `"double quotes`""
```

PowerShell doubles an apostrophe inside single quotes and uses a backtick to
escape a quote inside double quotes. Other shells have their own escaping rules.

This is a metadata read, not a full-capture validation: traffic bodies are skipped
and are not integrity-checked. The listing includes `sealed`, `truncatedTail` and
`bodyIntegrityChecked: false`. A readable committed prefix can expose its saved
context, with a notice that later context may be unavailable. The reader bounds
context to 256 fields and 16 MiB in total; native metadata limits also apply.

## Export records or a SAZ archive

```powershell
.\transmog-cli.exe capture export --input .\support-trace.tmcap --format jsonl --output .\support-trace.jsonl
.\transmog-cli.exe capture export --input .\support-trace.tmcap --format saz --output .\support-trace.saz
.\transmog-cli.exe capture export --input .\support-trace.tmcap --format saz-extended --output .\support-trace-extended.saz
```

| Format | Output and fidelity |
| --- | --- |
| `jsonl` (default) | Native records as JSON lines; defaults to stdout (`--output -`). No output encryption. |
| `saz` | Client-facing request and response in the conventional Fiddler archive structure. Requires a file destination. |
| `saz-extended` | Conventional SAZ members plus Transmog evidence and source metadata. Requires a file destination. |

Exports accept sealed files and recoverable unsealed prefixes. They report
`SOURCE_SEALED`, `SOURCE_TRUNCATED_TAIL`, `EXPORTED_RECORDS`, and
`EXPORTED_BYTES` on stderr. Check source completeness before sharing.
File exports use a temporary file beside the destination and publish it only
after conversion succeeds. Existing destinations remain intact, and failed
conversions leave no partial destination. JSONL sent to stdout cannot provide
that file-publication guarantee.

Both SAZ modes preserve exchanges with missing heads using explicitly labeled
compatibility placeholders, and report `PLACEHOLDER_SESSIONS`. These are not
captured server responses. Keep the TMCap input for original evidence. Retained
body prefixes can be exported and are disclosed with
SAZ completeness flags and an `INCOMPLETE_BODIES` report. SAZ conversion is
limited to 1,000,000 exchanges and 256 MiB of retained body bytes per direction
per exchange. Conversion also retains decoded source records in memory;
large captures can require substantial memory even though inspection and
validation release each frame. See [SAZ compatibility](saz-compatibility.md)
for representation and archive limits.

### Input and output passwords

Encrypted native input prompts for a masked password when needed. New
encrypted output uses `--encrypt`, asks for a password and confirmation, and
never remembers it. For unattended runs, use protected UTF-8 password files
of at most 4096 bytes; trailing CR/LF characters are ignored. `context` also
accepts `--password TEXT` and supports `--no-prompt`, as described above.
Other commands do not accept password strings as options. Protect and retain the password
files yourself; passwords cannot be recovered.

| Operation | Input password file | Output password file |
| --- | --- | --- |
| `capture inspect`, `validate`, or `seal` | `--password-file` or `--source-password-file` | `seal` preserves input encryption and password. |
| `capture export` without `--encrypt` | `--password-file` or `--source-password-file` | Output is unencrypted. |
| `capture export --encrypt` | `--source-password-file` | `--password-file`; encrypted output must be SAZ. |
| `record --encrypt` | No input capture. | `--password-file` |

For example, export encrypted TMCap to encrypted SAZ with separate passwords:

```powershell
.\transmog-cli.exe capture export --input .\support-trace.tmcap --source-password-file .\input-password.txt --format saz --output .\protected-trace.saz --encrypt --password-file .\output-password.txt
```

Keep passwords separate when sharing encrypted traces. SAZ member names and
file sizes remain visible. JSONL output contains unencrypted records even when
the native input was encrypted.

## Use the CLI in scripts

Successful commands and help exit with code `0`; errors exit with code `2`
and a `transmog-cli:` message on stderr. In PowerShell, check `$LASTEXITCODE`
after invoking the binary. Choose explicit new output paths, provide password
files for encrypted operations, and choose a root-installation option for
unattended `record` runs. OS consent or `sudo` may still need interaction.

## Support profiles

`profile validate --input FILE [--format text|json]` validates a versioned support
capture profile without setup or writes. `record --profile FILE` applies its
run-scoped suggestions; explicit CLI flags win. Interactive `m` / `m NOTE` records
incident markers. See [the support profile contract](support-capture-profiles.md).
