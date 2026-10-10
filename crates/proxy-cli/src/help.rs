//! Contextual help, resolved before any certificate, proxy or file side effects.

use std::io;

pub(crate) fn requested(arguments: &[String]) -> io::Result<Option<&'static str>> {
    if arguments.is_empty() {
        return Ok(Some(ROOT));
    }
    if matches!(arguments[0].as_str(), "help" | "--help" | "-h") {
        return topic(&arguments[1..]).map(Some);
    }
    let depth = if matches!(arguments[0].as_str(), "ca" | "capture" | "roots")
        && arguments.get(1).is_some_and(|arg| !arg.starts_with('-'))
    {
        2
    } else {
        1
    };
    let mut index = depth;
    while let Some(argument) = arguments.get(index) {
        if matches!(argument.as_str(), "--help" | "-h") {
            return topic(&arguments[..depth]).map(Some);
        }
        index += if takes_value(argument) { 2 } else { 1 };
    }
    Ok(None)
}

fn takes_value(argument: &str) -> bool {
    matches!(
        argument,
        "--output"
            | "--listen"
            | "--route"
            | "--upstream-ca-cert"
            | "--request-body-limit"
            | "--password-file"
            | "--source-password-file"
            | "--circular-buffer"
            | "--ca-cert"
            | "--ca-key"
            | "--proof-id"
            | "--capture"
            | "--cert"
            | "--key"
            | "--name"
            | "--identity"
            | "--days"
            | "--input"
            | "--format"
    )
}

fn topic(path: &[String]) -> io::Result<&'static str> {
    let path = path.iter().map(String::as_str).collect::<Vec<_>>();
    match path.as_slice() {
        [] => Ok(ROOT),
        ["record"] => Ok(RECORD),
        ["serve"] => Ok(SERVE),
        ["roots"] | ["roots", "cleanup"] => Ok(ROOTS),
        ["ca"] => Ok(CA),
        ["ca", "generate"] => Ok(CA_GENERATE),
        ["ca", "protect"] => Ok(CA_PROTECT),
        ["ca", "issue"] => Ok(CA_ISSUE),
        ["capture"] => Ok(CAPTURE),
        ["capture", "inspect"] => Ok(INSPECT),
        ["capture", "validate"] => Ok(VALIDATE),
        ["capture", "seal"] => Ok(SEAL),
        ["capture", "export"] => Ok(EXPORT),
        _ => Err(crate::invalid_input(format!(
            "Unknown help topic {}; run transmog-cli help",
            path.join(" ")
        ))),
    }
}

const ROOT: &str = "transmog-cli — traffic capture and explicit proxy tools

Usage: transmog-cli COMMAND [OPTIONS]

Commands:
  record            Guided support recording; Ctrl+C stops and saves a TMCap trace
  roots cleanup     Restore interrupted Windows proxy settings and remove CLI roots
  serve             Run a proxy with an existing CA; host settings are configured manually
  ca generate       Generate a CA with an OS-user-protected private key
  ca protect        Protect an existing CA key for the current OS user
  ca issue          Issue a short-lived server certificate and PEM key
  capture inspect   Summarize a native capture, including its recoverable prefix
  capture validate  Check integrity and require a complete final seal
  capture seal      Recover an unsealed native capture to a new file
  capture export    Export native records as JSONL, SAZ or extended SAZ

Options:
  --help, -h        Show help; also accepted after any command
  --version, -V     Print the CLI version

Use transmog-cli COMMAND --help or transmog-cli help COMMAND for full options.
For grouped commands: transmog-cli help capture export
Existing destination files are never overwritten.
Support recording guide: https://github.com/erik-anderson/transmog/blob/main/docs/cli-support-capture.md";

const RECORD: &str = "Record a support trace

Usage: transmog-cli record [OPTIONS]

  --output FILE              New .tmcap path (default: Transmog-<timestamp>.tmcap in the current directory)
  --listen IP:PORT           Listener (default: 127.0.0.1:0; port 0 chooses a free port)
  --allow-remote             Permit non-loopback listening and remote clients
  --route auto|h1|h2|h3       Upstream protocol (default: auto)
  --upstream-ca-cert FILE    Add a PEM CA bundle to the operating system's upstream trust
  --install-root             Authorize root installation without a console question; OS consent still applies
  --no-install-root          Use manual HTTPS certificate setup
  --persistent-root          Keep and reuse an OS-user-protected CLI root (default: fresh root, key in memory)
  --no-system-proxy          Leave Windows proxy settings for manual configuration
  --redact                   Redact Authorization, Proxy-Authorization, Cookie and Set-Cookie values
  --retain-sensitive         Retain complete headers, including credentials and cookies (initial default)
  --request-body-limit BYTES Positive per-request retained byte limit (initial default: 25000000)
  --unlimited-request-bodies Remove the per-request capture cap; forwarding is independent of this cap
  --include-network-context  Include optional interface, DNS and route information in the trace
  --circular-buffer SIZE     Retain newest exchanges; auto, unlimited, bytes, MB, GB, MiB or GiB (at least 1 MiB)
  --encrypt                  Enable AES-256 password encryption; ask for and confirm a masked password
  --password-file FILE       UTF-8 password file for unattended encryption; requires --encrypt (up to 4096 bytes)
  --help, -h                 Show this help

Redaction and request-body limits persist across CLI recordings. Other options apply to this run.
Bodies, URLs and metadata can contain private data even with --redact; review before sharing.
Response bodies are recorded. Trace files have no fixed size ceiling.
Without --circular-buffer, records stream to disk and an interrupted file can be recovered.
Circular auto uses half installed RAM; limits above that and unlimited use disk. Memory buffers
are unsaved until stopping and are lost if the process is killed. A failed save retains a recovery file.
Passwords are never remembered; protect password files yourself. Passwords cannot be recovered.

Without an installation option, a console asks for consent; unattended runs must choose one.
Windows configures and restores the host proxy automatically unless --no-system-proxy is used.
Other platforms require manual HTTP/HTTPS proxy configuration. Remote Windows listeners also
require --no-system-proxy. Apps may have separate certificate stores or certificate pinning.
Ctrl+C once restores routing and drains active requests. Ctrl+C again stops unfinished work.

Example: transmog-cli record --output support.tmcap --redact --encrypt";

const SERVE: &str = "Run an explicit proxy

Usage: transmog-cli serve --ca-cert FILE --ca-key FILE [OPTIONS]

  --ca-cert FILE           PEM interception CA certificate
  --ca-key FILE            Matching protected or legacy PEM CA key; legacy keys are protected on load
  --upstream-ca-cert FILE  Add a PEM CA bundle to operating-system upstream trust
  --listen IP:PORT         Listener (default: 127.0.0.1:0; port 0 chooses a free port)
  --allow-remote           Permit non-loopback listening and remote clients
  --route auto|h1|h2|h3     Upstream protocol (default: auto)
  --proof-id ID            Verification marker: add x-intercept-test, request identity encoding,
                          and add x-intercepted-by plus an escaped meta marker to HTML responses.
                          Changes traffic; HTML processing buffers at most 16 MiB. No control characters.
  --capture FILE          Stream a native capture to a new file; sensitive headers are redacted
  --capture-bodies        Include body bytes; requires --capture (request retention capped at 25000000 bytes)
  --help, -h               Show this help

Configure clients' proxy and HTTPS certificate trust manually. No host proxy or root installation
is performed. Ctrl+C stops the proxy and seals an active capture.
For guided setup, encryption or circular retention, use record.";

const ROOTS: &str = "Clean up CLI-owned roots

Usage: transmog-cli roots cleanup [--include-persistent]

  --include-persistent  Also retire persistent roots and remove their protected keys
  --help, -h            Show this help

Restores interrupted Windows proxy settings before certificate prompts. Removes pending,
ephemeral and retired CLI roots; active persistent roots are retained by default.
Approve OS removal prompts if shown. Failed cleanup retains public recovery identities for retry.
State is separate from the desktop: Windows %LOCALAPPDATA%\\Transmog-cli,
macOS ~/Library/Application Support/Transmog-cli, Linux $XDG_STATE_HOME/Transmog-cli
(or ~/.local/state/Transmog-cli). Certificates manually installed in application-specific
trust stores must be removed from those stores manually.";

const CA: &str = "Certificate tools

Usage: transmog-cli ca generate|protect|issue [OPTIONS]

  generate  Create a public CA certificate and OS-user-protected private key
  protect   Protect a validated legacy CA key without changing its certificate identity
  issue     Issue a server certificate and unencrypted PEM leaf key

Use transmog-cli ca COMMAND --help for full options.";

const CA_GENERATE: &str = "Generate an interception CA

Usage: transmog-cli ca generate --cert FILE --key FILE [--name NAME]

  --cert FILE  New public PEM certificate path
  --key FILE   New OS-user-protected private key path
  --name NAME  Certificate name (default: Transmog local interception CA)
  --help, -h   Show this help

The CA is valid for 365 days. Install only the public certificate; trust installation is manual.
Existing files are never overwritten. Keys are bound to the current OS user and credential store.";

const CA_PROTECT: &str = "Protect an existing CA key

Usage: transmog-cli ca protect --ca-cert FILE --ca-key FILE

  --ca-cert FILE  PEM CA certificate used to validate the matching key
  --ca-key FILE   Existing private key to protect in place for the current OS user
  --help, -h      Show this help

Already protected keys are verified. Protection failures preserve existing material.
On Linux, OS key protection requires a working Secret Service.";

const CA_ISSUE: &str = "Issue a server leaf certificate

Usage: transmog-cli ca issue --ca-cert FILE --ca-key FILE --identity HOST_OR_IP --cert FILE --key FILE [--days N]

  --ca-cert FILE       PEM signing CA certificate
  --ca-key FILE        Matching protected or legacy PEM CA key; legacy keys are protected on load
  --identity HOST_OR_IP DNS hostname or IP address for the leaf certificate
  --cert FILE          New public PEM server certificate path
  --key FILE           New unencrypted PEM server key path; protect this file
  --days N             Validity from 1 to 30 days (default: 7)
  --help, -h            Show this help

Existing output files are never overwritten.";

const CAPTURE: &str = "Native capture tools

Usage: transmog-cli capture inspect|validate|seal|export [OPTIONS]

  inspect   Print counts, retained bytes, seal and truncated-tail state
  validate  Check every frame and body payload and require a final seal
  seal      Write an unsealed capture's complete records to a new native file
  export    Write JSONL, SAZ or extended SAZ

Encrypted input prompts for a masked password or accepts --password-file FILE.
Use transmog-cli capture COMMAND --help for full options.";

const INSPECT: &str = "Inspect a native capture

Usage: transmog-cli capture inspect --input FILE [--password-file FILE]

  --input FILE                 Native TMCap input; sealed or recoverable unsealed prefix
  --password-file FILE         Protected UTF-8 password file for encrypted input (up to 4096 bytes)
  --source-password-file FILE  Alias for the input password file
  --help, -h                   Show this help

Without a password file, encrypted input asks in a console. Prints format revision, records,
exchanges, loss markers, retained body bytes, seal state, truncated tail and valid prefix bytes.
Checks one frame at a time, including body integrity, without retaining all body payloads.";

const VALIDATE: &str = "Validate a native capture

Usage: transmog-cli capture validate --input FILE [--password-file FILE]

  --input FILE                 Native TMCap input
  --password-file FILE         Protected UTF-8 password file for encrypted input (up to 4096 bytes)
  --source-password-file FILE  Alias for the input password file
  --help, -h                   Show this help

Without a password file, encrypted input asks in a console. Checks every frame and body payload
one at a time. Prints VALID=true only for a sealed file without a truncated tail. An unsealed
capture needs capture seal --input FILE --output NEW_FILE first. Missing bytes cannot be recovered.";

const SEAL: &str = "Recover and seal an interrupted native capture

Usage: transmog-cli capture seal --input FILE --output NEW_FILE [--password-file FILE]

  --input FILE                 Unsealed native TMCap input; already sealed inputs are rejected
  --output NEW_FILE            New native destination; the input is never modified
  --password-file FILE         Protected UTF-8 password file for encrypted input (up to 4096 bytes)
  --source-password-file FILE  Alias for the input password file
  --help, -h                   Show this help

Without a password file, encrypted input asks in a console. Preserves input encryption and
writes complete records, omitting an interrupted tail. Missing bytes cannot be reconstructed.
The destination is published only after the recovered file is sealed successfully.";

const EXPORT: &str = "Export a native capture

Usage: transmog-cli capture export --input FILE [--format jsonl|saz|saz-extended] [--output FILE|-] [OPTIONS]

  --input FILE                 Native TMCap source; unsealed prefixes can also be exported
  --format FORMAT              jsonl (default), saz (compatibility), or saz-extended (extra evidence/source metadata)
  --output FILE|-              New file or stdout '-' (default: stdout; SAZ requires a file)
  --encrypt                    AES-256 encrypted SAZ output; asks for and confirms a masked password
  --password-file FILE         Input password without --encrypt; output password with --encrypt
  --source-password-file FILE  Input password when output is encrypted or a separate password is needed
  --help, -h                   Show this help

Password files contain UTF-8 text, at most 4096 bytes; a trailing newline is ignored.
Without input password files, encrypted input asks in a console. Protect password files yourself.
JSONL has no encryption option. Passwords cannot be recovered and are never saved as preferences.
SAZ rejects exchanges missing either HTTP head; keep TMCap to preserve them. Incomplete bodies
are reported. SAZ conversion is limited to 256 MiB retained per direction and 1000000 exchanges.
Existing destinations are never overwritten; failed conversions leave no destination file.
Export reports and source completeness go to stderr; stdout JSONL contains only records.

Example: transmog-cli capture export --input source.tmcap --source-password-file input.txt --format saz --output encrypted.saz --encrypt --password-file output.txt";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_resolves_every_command_and_password_context() {
        for command in [
            "record",
            "serve",
            "roots cleanup",
            "ca generate",
            "ca protect",
            "ca issue",
            "capture inspect",
            "capture validate",
            "capture seal",
            "capture export",
        ] {
            let mut arguments = command
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            arguments.push("--help".into());
            let text = requested(&arguments).unwrap().unwrap();
            assert!(text.contains("Usage:"));
            let mut explicit = vec!["help".into()];
            explicit.extend(arguments[..arguments.len() - 1].iter().cloned());
            assert_eq!(requested(&explicit).unwrap(), Some(text));
        }
        assert!(EXPORT.contains("--source-password-file"));
        assert!(EXPORT.contains("--encrypt"));
        assert!(RECORD.contains("persist"));
        assert!(RECORD.contains("--upstream-ca-cert"));
        assert!(requested(&["help".into(), "missing".into()]).is_err());
    }

    #[test]
    fn help_like_option_values_are_not_help_requests() {
        assert_eq!(
            requested(&["serve".into(), "--proof-id".into(), "-h".into()]).unwrap(),
            None
        );
        assert_eq!(
            requested(&["record".into(), "--no-install-root".into(), "-h".into()]).unwrap(),
            Some(RECORD)
        );
    }
}
