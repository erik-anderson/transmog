# Transmog naming

The product and project brand is **Transmog**. User-visible prose, window
titles, certificates, installer metadata, and diagnostics use that exact
capitalization.

Developer-facing identifiers follow their ecosystem conventions:

- the command-line executable is `transmog-cli` (`transmog-cli.exe` on Windows),
  and its Cargo package is `transmog`;
- library packages use the `transmog-*` prefix and Rust imports use
  `transmog_*`;
- the desktop Cargo package is `transmog-desktop`, and its Windows executable
  is `transmog.exe`;
- npm workspace packages use the `@transmog` scope;
- test configuration uses `TRANSMOG_*` environment variables;
- the embedded application origin is `transmog-ui://localhost`, mapped by
  WebView2 to `http://transmog-ui.localhost`;
- the Tauri application identifier is `app.transmog.desktop`;
- Windows application state, diagnostics, CA material, and WebView2 data live
  under the single user-visible `%LOCALAPPDATA%\Transmog` folder. The package
  identifier must not be used as an application-data folder name.

Native captures use the `.tmcap` extension and `TMCAP01` preamble. Extended SAZ
exports use `transmog/manifest.json` and the `transmog-saz-extension-v1` format
marker. Pre-Transmog development formats are intentionally unsupported.

The source repository retains its existing GitHub URL until the remote
repository is renamed. Package metadata must continue to point at a real URL;
the URL is not a user-visible product name.
