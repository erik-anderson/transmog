import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
const compiled = await build({
  entryPoints: [fileURLToPath(new URL("../src/markers.ts", import.meta.url))],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { markerGroups, traceMarkers, markerTime } = await import(
  "data:text/javascript;base64," + Buffer.from(compiled.outputFiles[0].text).toString("base64")
);
const marker = (id, time) => ({
  id,
  runId: "run",
  unixMillis: time,
  offsetMillis: null,
  origin: "cli",
  creationKind: "user-mark",
  note: "",
});
test("marker positions include endpoints and collect crowded flags into a chooser", () => {
  const items = traceMarkers([
    {
      traceId: "source",
      name: "Customer trace",
      markers: [marker("first", 1000), marker("near", 1001), marker("last", 2000)],
    },
  ]);
  const groups = markerGroups(items, 1000, 2000, 600);
  assert.equal(groups.length, 2);
  assert.equal(groups[0].markers.length, 2);
  assert.equal(groups[0].label, "2 markers");
  assert.equal(groups[1].markers[0].id, "last");
  assert.equal(markerGroups(items, 1200, 1800, 600).length, 0);
});
test("repeated marker identities stay source scoped and exact message links remain explicit", () => {
  const items = traceMarkers([
    {
      traceId: "first",
      name: "First trace",
      markers: [marker("same", 1000)],
      markerEntries: { same: "request-1" },
      links: [{ markerId: "same", boundary: "response" }],
    },
    { traceId: "second", name: "Second trace", markers: [marker("same", 1000)] },
  ]);
  assert.notEqual(items[0].key, items[1].key);
  assert.equal(items[0].entryId, "request-1");
  assert.equal(items[0].boundary, "response");
  assert.equal(items[1].entryId, null);
  assert.match(items[1].description, /Second trace/);
});
test("recorded offsets use readable units and missing offsets retain an absolute time", () => {
  assert.equal(markerTime({ ...marker("id", 1000), offsetMillis: 83000 }), "1m 23s into capture");
  assert.doesNotMatch(markerTime(marker("id", 1000)), /into capture/);
});
