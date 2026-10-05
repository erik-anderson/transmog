# Safe response previews

Transmog treats every captured response body as hostile input. Text and byte
views are assigned as text, never parsed as markup. Image previews cross an
additional process and origin boundary before the WebView can display them.

## Allowed image path

The initial image allowlist is PNG, JPEG, GIF, and WebP. The application reads
only a complete retained body, removes supported HTTP content codings under the
content layer's expansion limits, and rejects encoded or decoded sources over
16 MiB. Signature detection must select one of the allowlisted raster formats;
the media-type header is not trusted.

`transmog-preview-worker` is a one-shot helper. A trusted bootstrap creates a
unique ephemeral, zero-capability AppContainer, assigns its suspended child to
a kill-on-close Job Object with a 384 MiB process-memory limit and all Job UI
restrictions, then resumes it. The child verifies both boundaries before
reading its bounded standard-input frame. It has no network, filesystem,
registry, clipboard, window, child-process, or certificate capability.

The decoder accepts dimensions no larger than 8192 by 8192, at most 40 million
pixels, and a 256 MiB decoder allocation budget. Only one decoded frame is
used, so animation and subsequent frames are discarded. The result is
re-encoded as a metadata-free PNG and capped at 64 MiB. A three-second
application deadline kills the helper on timeout. A decoder crash, truncated
protocol, malformed image, resource violation, or nonzero exit yields an
explicit unavailable preview; it cannot terminate the desktop or proxy.

Normalized PNGs enter a 64-entry, 256 MiB FIFO memory cache and receive random
opaque handles. They are served only from the `transmog-preview` custom origin
with `default-src 'none'`, `nosniff`, `no-store`, and no-referrer headers. The
main origin permits that image origin but cannot insert captured HTML, SVG,
scripts, styles, fonts, PDF, audio, video, or documents. Preview navigation is
not allowlisted.

## Qualification

Unit tests cover every allowlisted format, active-content polyglots, malformed
signatures, source and dimension limits, and bounded framing. Windows tests run
the real decoder in its AppContainer. Application tests cover opaque-handle
validation and deterministic cache eviction. The standalone interop suite
exercises preview creation while real clients and origins communicate through
the proxy.

Adding another format requires an explicit threat analysis, decoder and
sandbox review, hostile corpus, resource-limit tests, CSP/origin review, and an
accessible fallback before it enters the allowlist.
