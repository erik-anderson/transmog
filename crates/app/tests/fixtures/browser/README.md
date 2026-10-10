# Chromium archive fixtures

`placeholder.netlog` and `placeholder.har` were captured with Chromium
153.0.8010.12 in a fresh headless profile, loading a controlled local HTML page
and its stylesheet. The archives retain actual Chromium event types, request
and response headers, source dependencies, timings, and response bytes.

IP addresses use reserved documentation ranges. Machine DNS/proxy snapshots,
browser command lines, profile paths, and field-trial settings are removed or
replaced. Capture clocks use a synthetic 2026-01-01 epoch; generated page/frame
identifiers and the local server port are normalized. Credential headers and
cookies are rejected before the files are written.

The Rust import regression uses these files without launching a browser or
accessing the network. To regenerate them using the repository's installed
Playwright Chromium:

```powershell
node scripts/generate-browser-archives.mjs
node --test scripts/sanitize-browser-archives.test.mjs
```
