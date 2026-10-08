# Safe response previews

Transmog treats every captured response body as hostile input. Workbench text
and byte views are assigned as text, never parsed as markup. Images are displayed only
through an `<img>` element using an opaque resource on the isolated preview
origin. Raster images also cross a process sandbox before reaching that origin.

The explicit [captured-page preview](captured-page-preview.md) is a separate
Windows browser window with its own profile, native resource interception and
warning/script-choice flow. It does not render captured HTML in the workbench
or grant access to application commands.

## Allowed image path

The raster allowlist is PNG, JPEG, GIF, and WebP. SVG is allowed only when the
response media type is `image/svg+xml`, the body is valid UTF-8, and an SVG root
is present. The application reads only a complete retained body, removes
supported HTTP content codings under the content layer's expansion limits, and
rejects encoded or decoded sources over 16 MiB. Signature detection, rather
than the media-type header, selects a raster decoder.

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
opaque handles. Validated SVG bytes use the same bounded opaque cache directly;
they are not parsed into application markup or passed through the raster
decoder. Both forms are served only from the `transmog-preview` custom origin
with `default-src 'none'; sandbox`, `nosniff`, `no-store`, and no-referrer
headers and are loaded only by `<img>`. The main origin cannot insert captured
HTML, SVG markup, scripts, styles, fonts, PDF, audio, video, or documents.
Preview navigation is not allowlisted.

## Qualification

Unit tests cover every allowlisted raster format, SVG qualification, active-
content polyglots, malformed signatures, source and dimension limits, and
bounded framing. Windows tests run the real raster decoder in its AppContainer.
Application tests cover opaque-handle validation and deterministic cache
eviction. The standalone interop suite exercises preview creation while real
clients and origins communicate through the proxy.

Adding another format requires an explicit threat analysis, decoder and
sandbox review, hostile corpus, resource-limit tests, CSP/origin review, and an
accessible fallback before it enters the allowlist.
