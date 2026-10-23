# Authentication inspection

Choose **Auth** beside Headers, Body, Split and Raw data on the selected request
or response. Each side remembers its view independently and follows its selected
original/effective stage. The badge counts top-level artifacts, including opaque
or malformed ones; nested decoders do not inflate it. The tab appears when the
selected message has findings and stays hidden when automatic detection finds
nothing. Discovery never selects Auth or moves focus. An inherited Auth view
returns to Headers or Body when the next message has no findings.
If discovery fails, **Auth unavailable** stays available for a retry; an error
is not reported as an empty result.

## Sources and interpretation

Discovery uses complete retained headers and request targets, including
duplicates and fields beyond the displayed page. Captured Location queries and
fragments are eligible; browser fragments absent from HTTP requests cannot be
recovered. Live and imported traffic use the same native analysis path.

Automatic detection also scans an eligible complete retained body in each visible
selected message. Other rows and routine list refreshes do not read bodies.
Failures offer **Retry inspection** directly. After a successful scan, use
**About this inspection → Refresh inspection** to repeat it. This disclosure near
the top of Auth explains the shared verification and privacy limits once. The
persistent overview distinguishes complete from limited source coverage;
**Source coverage** contains the individual source details.

For a body that is itself a standalone token or SAML document, choose **Body**,
then select JWT/JWS/JWE, SAML XML, NTLM or Negotiate from the **Authentication ·
entire body** group in the body viewer menu. These choices interpret the complete
decoded body, rather than selected text or a token embedded in a JSON response.
JSON/form/HTML auth fields are detected automatically.

The Auth view identifies the requested format and reports a recognized body,
no match, or the retention/decoding reason it could not read the body. Header and
URL findings stay visible. **Back to body** restores the body viewer and automatic
detection. Leaving Auth or changing exchanges/stages also resets the explicit
format. Unavailable or incomplete bodies disable these format choices. Explicit
interpretation retains the same masking and resource limits and verifies no
credentials or signatures.

| Evidence | Readable interpretation |
| --- | --- |
| HTTP envelopes | Multiple challenges, quoted commas/escapes, duplicate parameters, proxy/origin role, unknown schemes and valid bare Negotiate advertisements. Successful responses can carry final tokens too. Authentication-Info retains its original scheme-free source. |
| Basic / Digest / Bearer | Basic splits at the first colon, retains lossless decoded bytes and explains non-ASCII ambiguity. Digest shows parameters without validating response hashes. Opaque Bearer values are supported; Bearer alone does not establish OAuth or JWT. |
| OAuth / OIDC | Authorization requests, callbacks, token grants/responses, client/redirect/scope, PKCE and contextual token roles. Generic JSON code/state/error alone is insufficient. Codes, state, nonce and verifiers remain opaque. Relative expires_in does not become an invented absolute expiry. |
| JWT / JWS / JWE | JOSE metadata, JSON claims and exact numeric spellings. Duplicate members remain ambiguous; unsupported critical extensions are explicit. Non-JSON payload bytes require reveal. JWE exposes header/component lengths; its claims remain encrypted. |
| SAML 2.0 | Redirect raw-DEFLATE, POST/form values, namespace-aware XML/SOAP, inert hidden HTML fields, binding metadata and opaque artifacts. Available issuer, status, conditions, subject/attributes and signature/encryption metadata are unverified. |
| NTLM Types 1/2/3 | Named/raw flags, available domain/user/workstation/target, version metadata, AV pairs/FILETIME, response lengths and justified NTLMv2-style structure. Unknown OEM code pages retain non-ASCII hex bytes. |
| GSS-API / SPNEGO | Wrapper OIDs, offered versus explicitly selected mechanisms, negotiation state/flags, MIC presence and confirmed children. Kerberos offered does not mean Kerberos observed. |
| Kerberos V5 | Direct/wrapped AP-REQ, AP-REP and recognizable errors; exposed protocol/ticket versions, outer realm/service principal, encryption type and key version. Ticket/authenticator/reply contents remain encrypted. |

Automatic body extraction supports form URL encoding, JSON/`+json`, XML/SOAP/
`+xml` and named hidden HTML inputs. Captured HTML stays inert; scripts never
execute. Multipart/file parts and arbitrary binary bodies are not automatically
scanned. HTTP decoding uses the existing bounded codecs. UTF-8/ASCII and
BOM-selected UTF-16 are supported; unsupported/conflicting charsets or content
metadata are reported. XML DTDs/external entities and indefinite ASN.1 lengths
are rejected. No captured URL, schema, key, certificate, artifact or discovery
endpoint is fetched.

## Trust, privacy and provenance

Decoding and trust are separate. **Not checked** means no signature, MIC, NTLM
response or cryptographic authentication was verified. Type 3, accept-completed
and challenge messages do not prove successful login. Kerberos client identity,
keys, ticket lifetime and authorization data generally remain encrypted.

JWT NumericDate and SAML condition annotations compare with recorded exchange
time when available. Missing capture time is explicit. Raw timestamps remain
available, including fractional or large numbers. Missing claims are not
automatically invalid, and an expired-at-present token may have been within its
conditions at capture. All claims remain unverified.

Comparisons use whole-second recorded exchange time. Ordering inside that
one-second boundary is explicitly uncertain; raw fractional values are preserved.

Passwords, client secrets, tokens/codes/correlation values, PKCE verifiers,
raw/decoded representations and cryptographic bytes are masked by default.
The eye button beside a value reveals or hides that field. Right-click or use
Shift+F10 for **Copy value**, which copies the actual authorized retained contents
even when masked. **Technical details** groups decoding provenance and raw/decoded
material after the readable fields and decoded children; values still require
reveal. Field-specific interpretation notes, such as expiry-time comparisons,
appear beside their values without another disclosure. Ordinary JSON and SAML
fields do not repeat the shared unverified-value caveat. Single-child envelopes
that refer to the same retained source are presented as one artifact, such as
JWT/JWS within Bearer. The technical details retain each decoder layer, role,
status and source range. Distinct source
ranges and sibling artifacts stay separate, warnings remain attributed to their
layer, and subtree copying preserves the original evidence hierarchy. JWT claims
lead the readable fields; JOSE metadata follows them without changing captured
field order in subtree copies.
User/domain/workstation and ordinary claim
metadata are readable diagnostic evidence and may contain personal data. Reveals
reset on selection, stage/evidence changes, hide, disconnect or workspace change;
neither reveal state nor values become preferences or diagnostic history.
Redacted values cannot be reconstructed.

Right-click fields for name/value actions and finding headings for subtree actions.
Copy subtree masks sensitive fields regardless of reveal state. Copy subtree
including secrets explicitly includes available actual values. See
[copying inspection values](copying-inspection-values.md).

**Show original source** opens the original header occurrence or enclosing body,
which can contain unmasked secrets. Provenance records HTTP, URL/form, character,
Base64, DEFLATE and protocol transformations. Derived XML regions do not claim
exact compressed-byte mapping. Display masking does not sanitize traffic or
extend capture/export redaction to URLs, bodies or additional headers. See
[retention and privacy](product-state-and-support.md#retention-and-privacy).

## Coverage, bounds and related evidence

Coverage distinguishes complete scans, skipped/unsupported types, pending or
incomplete capture, disabled/omitted/evicted content, redaction and parser limits.
A negative completed scan says nothing about unscanned sources. Malformed
artifacts do not suppress valid siblings. Native owner, document, generation,
evidence revision and retention checks protect explicit reads; obsolete results
cannot overwrite a newer view or restore a reveal.

Finite bounds include 1 MiB per encoded candidate/explicit interpretation,
2 MiB automatically read encoded body, 4 MiB decoded HTTP/SAML output and 8 MiB
aggregate analysis/value material. An inspection has at most 64 top-level
artifacts, 2,048 fields, depth 24 and 100,000 cooperative work steps. JSON has
2,048 syntax nodes, HTML has 256 hidden controls and 128 attributes per tag, and
NTLM target-info has 256 AV pairs. Public field previews shorten at 8 KiB;
explicit value retrieval remains bounded. Exhaustion means partial coverage.

**Related traffic** searches up to the latest 512 retained exchanges, 2 MiB of
candidate header/target data and 100 results. It reads no other bodies. Results
stay in the owning workspace/capture namespace. OAuth requires correlation plus
client/redirect context; SAML requires observed ID/InResponseTo plus issuer or
destination context. A captured redirect can link to its observed destination.
Connection neighbors use observed client-leg identity, authority and proxy/origin
role; shared or multiplexed connections are context, not proof of a handshake.
Missing imported connection metadata is reported instead of substituting host/
time proximity. Body-only relationships can be absent from this searched scope;
the view does not reconstruct a complete login flow.

**Related traffic** shows the searched scope and any search limit above the source
fields. **Back to evidence** restores the position from before the search.
After opening a related request, **Back to auth source** returns to the original
message stage, body interpretation and reading position. Reveals reset normally;
return navigation does not restore credentials into the display. A navigation
that is superseded by a new focus or selection does not steal focus.
For WebSocket and event-stream exchanges, both directions open the HTTP
handshake inspector so the requested Auth evidence is visible.
