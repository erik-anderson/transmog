# Minimal visualizer extension

Copy this folder to start a read-only extension in an external editor. It contains
plain JavaScript and needs no compiler, package manager or build step. Its single
visualizer displays the retained HTTP response status. It declares no header or
body access and never modifies traffic.

1. Change the extension `id` and `name` in `manifest.json` for your project.
   Each loaded extension needs a unique ID; the visualizer ID is local to it.
2. In **Settings → Extensions**, choose **Load unpacked…** and select your copy.
3. Select a retained request with a response and a request path beginning with
   `/`. In the response inspector's **Headers** view, select **Response overview**.
4. Edit `main.js`. In Extensions settings, choose **Reload active visualizers**
   or enable **Hot reload active visualizers**. The last good result remains
   visible if an edit fails. Reloading does not open an inactive visualizer.
5. When ready to share, choose **Package extension…** on the unpacked entry.
   Recipients install the resulting `.tmext` from Extensions settings.

The `files` inventory lists every file available to the worker. Add any imported
JavaScript modules there; imports must be static and package-relative. This README
is not inventoried, so it is not included in the package.

Unpacked file entries may omit both `bytes` and `sha256`. Transmog calculates
current lengths and hashes when collecting sources and packaging; editing a file
does not require metadata updates. Distributed `.tmext` packages contain exact
lengths and hashes. Use **Package extension…** to generate that metadata and the
archive format.

To read headers or bodies, update the exact input declarations and native matching
rules before adding code that consumes them. Handlers are synchronous ES2022
module exports; there is no `fetch`, filesystem access or CommonJS `require`.
TypeScript is optional and must be compiled to JavaScript externally.

See the [extension guide](https://github.com/erik-anderson/transmog/blob/main/docs/extensions.md)
and the optional [authoring types](https://github.com/erik-anderson/transmog/blob/main/crates/extensions/sdk/transmog-extensions.d.ts) for the
input, output, permission and module contracts.
