# Network profiles and connection settings

Use **Settings → Preferences → Network** to control connections made by Transmog
and Composer. **Decrypt HTTPS traffic** and **Profile** stay above the advanced
settings. Opaque CONNECT tunnels carry the original client's handshake, so their
TLS and HTTP offers are not controlled by a Transmog profile.

## Choose and activate a profile

**WebView2 (Chromium 154)** is the default. Its transport settings match
**Chromium 154**, qualified against 154.0.8037.99; the root-store source differs.
WebView2 discovers its installed component and Chromium discovers Chrome's.
Both fall back to the bundled Chromium store and add monitored local Windows
trust. Both enable H2/H3 ALPS and certificate-based Trust Anchor IDs.

Built-ins declare fixed settings and retain their qualified browser version.
Certificate-data updates refresh verification inputs without editing profiles.
Use **Customize** to create an editable copy; custom profiles retain authored
choices when built-ins or component data change.

Selecting a profile previews its settings. **Use profile** activates its saved
recipe. In the editor, **Save only** saves the definition without changing the
active revision; **Save and use** saves and activates it. Closing the editor
keeps its draft; **Discard draft** removes it. The profile menu supports editing,
duplication and deletion with Undo.

Activation creates fresh TLS contexts and origin pools for the proxy and
Composer, including already-open capture viewers. Admitted requests and streams
finish with their original recipe, which remains in their recorded evidence.

## Customize the offer

The main controls cover HTTP/1, HTTP/2, HTTP/3, TLS versions, compatibility
ciphers, hybrid key exchange, server connection reuse and root-store selection.
HTTP/3 requires TLS 1.3. Disabling all HTTP protocols intentionally blocks new
outbound work. Server reuse defaults on; disabling it gives subsequent requests
dedicated H1/H2/H3 connections, including viewer Composer.

**More handshake options** contains ordinary and signature GREASE, extension
order, real ECH, ECH GREASE, certificate compression, separate H2/H3 ALPS choices,
Trust Anchor IDs and an optional full path to `crs.pb`. ECH GREASE sends a dummy
extension and does not encrypt the server name. Trust Anchor IDs are independent
of PQC algorithm choices.

**Profile details** explains the Client Hints limitation: Transmog applies ALPS
transport settings and records server preferences, but leaves hint values and
permissions with the caller. It does not fabricate hints or retry requests to
satisfy connection-level preferences.

See [negotiation](chromium-negotiation.md) for precise offers and retry behavior,
and [ALPS](alps.md) for transport settings and captured contents.

## Choose certificate trust

Customize offers the public OS store, bundled Chromium roots, installed Chrome
roots or installed WebView2 roots. Installed sources fall back to the bundle.
The selection changes actual origin certificate trust independently of the
Trust Anchor ID switch. Every choice uses the same [verifier](chromium-verifier.md).

**Trust locally installed Windows certificates** sits beside the root selector.
Turning it off uses only the chosen public roots for trust grants. Windows user
and enterprise Disallowed certificates still apply. Bundled Chromium and Public
OS also enforce the full cached Windows untrusted CTL. Installed Chrome and
WebView2 keep Chromium's certificate-only local distrust scope, including when
their roots fall back to the bundle; they do not import Windows' public untrusted
CTL. OS mode already includes local roots, so this switch is disabled.
Component discovery does not install
certificates or modify Chrome/WebView2 files. If the selected component has no
supported IDs, Transmog omits the offer and never borrows another store's IDs.

An explicit `crs.pb` replaces the bundled baseline; a newer valid component from
the chosen installed source takes precedence. In OS mode it supplies only ID
metadata and untrusted issuer helpers. Save and use loads the file before
activation and rejects invalid input when used for trust or IDs. If a saved
file disappears, inspection and Settings remain usable while sends and proxy
startup explain repair. Save and use reloads a repaired file even when the
recipe is unchanged. See [root stores](root-stores.md) for discovery paths,
freshness, monitoring, recovery and provenance.

## Decryption and listener settings

**Decrypt HTTPS traffic** applies at the next proxy start. With it off, CONNECT
forwards opaque bytes without loading an interception CA or requiring CA trust.
Traffic and TMCap retain the destination, endpoints, outcome and byte counts;
the recorded HTTP protocol is the outer CONNECT protocol, and the tunneled
protocol and contents remain unknown. Decrypted entries retain their CONNECT
destination alongside inspected messages and TLS evidence.

**Advanced network settings** contains remote-client access, an optional proxy
port, caller-process isolation and [client-certificate authorization](client-certificates.md).
Remote access defaults off; an empty port chooses an available port at each
start. Authored ports from 1 through 65535 are retained, and an occupied port
produces an actionable start failure. Address and port changes apply to the next
fresh start. Windows routes through loopback even when remote clients are allowed.

Caller isolation defaults on, falls back to source IP and also separates
downstream H2 sessions. Embedders can supply their own isolation-key computation.
Saving these settings preserves active work and does not restart the proxy.

The ordinary desktop flow configures the current-user system proxy. An explicitly
configured `configureSystemProxy: false` binds for manual client routing without
changing Windows settings or requiring managed CA installation. Saving settings
preserves that choice. Decrypted operation still validates and protects CA files;
both modes block startup while host restoration is pending. See the
[desktop lifecycle](product-shell.md#start-and-stop) and
[preference persistence contract](product-state-and-support.md#persisted-state).
