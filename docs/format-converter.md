# Format converter

Use **Format converter** in the desktop top bar to encode, decode or hash text
without leaving the current workspace. It is available in the main window and
capture viewers regardless of proxy state. The flyout opens beside its button;
compact windows show the icon with an accessible name and tooltip.

Enter Input, choose a Conversion, and inspect the read-only Output. Choosing a
conversion and subsequent input edits update the output after a short delay.
**Convert** also runs it explicitly; **Cancel** stops the current work while
preserving input. Editing input or conversion choices, starting a conversion,
cancellation and failure clear obsolete output immediately. Copying, saving and
chaining become available when the current conversion succeeds. Late results
cannot replace newer input.

**Copy output** copies the displayed representation. **Use output as input**
preserves the result's bytes and encoding, using Hex input for binary results.
For authentication results it clears Conversion and prompts you to choose the
next conversion, so decoded JSON is ready for the next task. Other conversions
keep their operation for repeated chaining. Undo/Redo restores both input and
operation together.
This edit, input replacement and ordinary typing support Ctrl+Z/Ctrl+Y in the
input editor. **Save output…** saves complete output bytes through a native file
dialog, independently of the selected output presentation.

## Opening from evidence

Right-click selected text and choose **Open in Format converter**. The same
action is available through Shift+F10/the Menu key and in Monaco editor context
menus. Selection is copied explicitly; the tool does not read the clipboard
when it opens or change source evidence.

Right-click either column of a request/response header row to use that specific
occurrence's complete original value. A selection at the clicked location takes
precedence. Header order, duplicate fields, whitespace and empty values remain
distinct; display truncation does not truncate the converter input. Unavailable,
redacted and binary header values explain why they cannot be supplied as text.
Known authentication headers preselect Auto or the appropriate parameter parser
and decode immediately. Selecting text keeps ordinary text-selection behavior.

In **Auth**, right-click a supported artifact heading and choose **Open token in
Format converter**. This retrieves its exact native retained token, including
collapsed Bearer/JOSE and Windows envelopes, and preselects the native operation.
Field menus also offer **Open value in Format converter** for the complete actual
value. Masked previews are never used as converter input. Native authorization
rechecks the owning window, active evidence generation and retained source before
the handoff. Once copied, scratch input has no live capture dependency.

If the flyout already contains input, choose **Replace input**, **Open in new
window**, or **Cancel**. Replacing input, operation, representation and encoding
is one undoable edit; Cancel preserves the complete previous draft. Cancel,
Escape and closing the converter return keyboard focus to the original action's
source while it remains available, including an Auth artifact heading. Captured-page browser
previews remain isolated from application context actions.

## Independent windows and temporary work

**Open in new window** transfers a flyout draft to an independent resizable
native window. The source clears only after the destination receives it. A
failure leaves the source draft usable. The destination keeps its operation,
input, encoding and undo history; output is recomputed from that snapshot.
Each window owns its work and remains usable after its origin closes. There is
no fixed limit on converter-window count. In a standalone converter, **Open in
new window** copies its draft into another independent window.

Escape, Close and outside clicks dismiss the flyout while preserving its draft.
Escape/Close return focus to its invoker; outside clicks retain their destination
focus. Escape does not close standalone windows. Closing a standalone window
ends that scratch session. Contents are temporary and are not restored on
restart, saved in captures/preferences, placed in URLs/titles or recorded in
diagnostics. Normal app shutdown and software updates end converter sessions.

## Conversions and byte semantics

| Group | Operations |
| --- | --- |
| Base64 | Standard and URL-safe encode/decode |
| URL | Component encode/decode; separate form-value encode/decode |
| Hex | Bytes to/from hexadecimal |
| HTML | Encode/decode character references as inert text |
| Strings | Quote/unquote complete JSON strings |
| Authentication | Decode authentication (auto), Decode JWT, Decode JWS, Inspect JWE, Decode Basic, Parse Digest, Decode NTLM, Decode Negotiate / SPNEGO |
| Hashes | MD5, SHA-1, SHA-256, SHA-384, SHA-512 |

UTF-8 is the default encoding for text/byte boundaries. **Text encoding** exposes
UTF-16LE, UTF-16BE and Windows-1252. Encoding is strict: malformed sequences and
unrepresentable characters are not replaced silently. BOMs are preserved as
ordinary content and are never added automatically. Text and Hex input/output
representations are explicit; undecodable output and nonprinting control bytes
are displayed as Hex. Tab and line endings remain available as text.
URL/HTML/JSON escaping works on Unicode text independently of byte-encoding
choices. Hashes are one-way digests, displayed as lowercase hexadecimal text.

- Standard Base64 encoding includes padding; Base64url encoding omits it.
  Decoding accepts canonical padded or unpadded input with the matching alphabet,
  checks unused bits, and rejects whitespace and malformed padding.
- URL components use UTF-8 percent escapes and preserve literal `+` on decode.
  Form values encode spaces as `+` and decode `+` as space. Malformed percent
  escapes and non-UTF-8 decoded bytes are errors.
- Hex uses complete byte pairs. ASCII whitespace is accepted between pairs,
  including at the ends; whitespace within a pair and incomplete pairs fail.
- HTML decoding recognizes semicolon-terminated named and numeric references,
  including named references containing two code points. Unknown or unterminated
  references remain literal text. Invalid numeric Unicode values fail.
- JSON unquoting requires a complete quoted JSON string with valid escapes and
  paired surrogates. Empty input is valid for encodings and hashes; an empty JSON
  string is written as `""` when unquoting.
- **Decode JWT** accepts a complete compact token, optionally prefixed by
  `Bearer`, with surrounding ASCII whitespace. Output is readable JSON containing
  the header, payload, Base64url signature and `verification: "Not checked"`.
  Decoding never verifies signatures, fetches keys or establishes claim validity.
  JSON member order, duplicates and exact number spellings are preserved.
  Malformed tokens and non-object payloads fail; encrypted JWE claims cannot be
  decoded. This operation uses Unicode text and UTF-8 output, with a 1 MiB token
  limit and the Auth parser's depth and field limits.

Authentication conversions also accept a complete header line, such as
`Authorization: Bearer …`, or a scheme-prefixed value. Choose **Decode
authentication (auto)** explicitly to recognize supported HTTP envelopes,
multiple challenges, scheme-free Authentication-Info parameters, JOSE tokens,
and recognizable NTLM/GSS/SPNEGO/Kerberos structures. Arbitrary bare Base64 is
ambiguous; choose **Decode Basic**, **Decode NTLM** or **Decode Negotiate /
SPNEGO** when its format is known. Editing input keeps the chosen operation.
Pasted input works independently of captured traffic and the Auth view.

Output is readable JSON and explicitly includes temporary input credentials.
General authentication output carries format, structural status, ordered values,
decoded content, child structures, diagnostics and transformation provenance.
Duplicate Digest parameters and JSON members remain in their original order;
exact JSON number spellings are preserved. Missing context leaves the role
unknown. Opaque Bearer tokens and unsupported schemes are identified explicitly.
**Decode JWS** retains non-claims and binary payloads losslessly. **Inspect JWE**
shows the protected header and encrypted component lengths; claims remain
encrypted. **Decode Basic** splits at the first colon, displays ASCII directly
and preserves non-ASCII username/password bytes as Base64 with charset ambiguity
explained. Windows mechanism tokens retain original decoded bytes. No operation
fetches keys, decrypts, verifies, sends traffic or establishes successful login.
Malformed, truncated and limited conversions preserve input and disable actions
on the previous output. SAML scratch conversion is not currently included.

Each conversion is limited to 8 MiB of input bytes and 32 MiB of output bytes.
Hex editor text is additionally bounded to 24 MiB. Work runs off the UI thread;
two workers are available across windows, without an unbounded queue. If they
are occupied, **Convert** offers an explicit retry. Limits reject excess instead
of truncating output. Undo retains up to twenty edits within a bounded history.

Custom TypeScript formats, compression, file inputs and saving
output as traffic are outside v1. Conversion uses internal desktop commands;
it does not extend network protocols or the external control protocol.
