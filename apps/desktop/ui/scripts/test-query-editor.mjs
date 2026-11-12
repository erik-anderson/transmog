import assert from "node:assert/strict";
import { test } from "node:test";
import {
  editQueryParameter,
  formatQueryUrl,
  newQueryParameter,
  parseQueryUrl,
} from "../src/query-editor/query-parameters.ts";

test("opening query rows round-trips every authored URL byte", () => {
  for (const value of [
    "",
    "not yet a URL",
    "HTTP://Example.TEST:80/a/../b?x=%2f&x=%2F&space=one+two&flag&empty=&=value&&#fragment?kept",
    "https://example.test/path?",
    "https://example.test/path?#",
    "https://example.test/path#fragment?not-a-query",
    "https://example.test/path?bad=%E9&escape=%&utf8=caf%C3%A9&equals=a=b=c",
    " https://example.test/path?x=1 ",
    "/relative?&&",
  ]) {
    assert.equal(formatQueryUrl(parseQueryUrl(value)), value);
  }
});

test("query rows keep duplicates, order, flags, empty values and empty segments", () => {
  const { rows } = parseQueryUrl("https://example.test/?a=1&a=2&flag&empty=&=value&&");
  assert.deepEqual(
    rows.map(({ name, value, hasEquals }) => ({ name, value, hasEquals })),
    [
      { name: "a", value: "1", hasEquals: true },
      { name: "a", value: "2", hasEquals: true },
      { name: "flag", value: "", hasEquals: false },
      { name: "empty", value: "", hasEquals: true },
      { name: "", value: "value", hasEquals: true },
      { name: "", value: "", hasEquals: false },
      { name: "", value: "", hasEquals: false },
    ],
  );
});

test("a component edit encodes only its changed bytes", () => {
  const draft = parseQueryUrl(
    "HTTP://Example.TEST:80/a/../b?na%6De=one+two&repeat=%2f&repeat=%2F&flag&bad=%E9#keep?x",
  );
  assert.equal(draft.rows[0].name, "name");
  assert.equal(draft.rows[0].value, "one two");
  draft.rows[0] = editQueryParameter(draft.rows[0], "value", "a & café/#=+");
  assert.equal(
    formatQueryUrl(draft),
    "HTTP://Example.TEST:80/a/../b?na%6De=a%20%26%20caf%C3%A9%2F%23%3D%2B&repeat=%2f&repeat=%2F&flag&bad=%E9#keep?x",
  );
  draft.rows[1] = editQueryParameter(draft.rows[1], "name", "new key");
  assert.equal(draft.rows[1].rawValue, "%2f");
  assert.equal(draft.rows[1].rawName, "new%20key");
});

test("same decoded values retain original escapes and plus encoding", () => {
  const draft = parseQueryUrl("https://example.test/?na%6de=one+two&bad=%");
  for (const [index, row] of draft.rows.entries()) {
    draft.rows[index] = editQueryParameter(
      editQueryParameter(row, "name", row.name),
      "value",
      row.value,
    );
  }
  assert.equal(formatQueryUrl(draft), "https://example.test/?na%6de=one+two&bad=%");
});

test("editing flag values creates an equals sign while clearing retains it", () => {
  const draft = parseQueryUrl("https://example.test/?flag&empty=");
  draft.rows[0] = editQueryParameter(draft.rows[0], "name", "renamed");
  assert.equal(formatQueryUrl(draft), "https://example.test/?renamed&empty=");
  draft.rows[0] = editQueryParameter(draft.rows[0], "value", "yes");
  assert.equal(formatQueryUrl(draft), "https://example.test/?renamed=yes&empty=");
  draft.rows[0] = editQueryParameter(draft.rows[0], "value", "");
  assert.equal(formatQueryUrl(draft), "https://example.test/?renamed=&empty=");
  draft.rows[0].hasEquals = false;
  assert.equal(formatQueryUrl(draft), "https://example.test/?renamed&empty=");
});

test("new query fields encode delimiters and incomplete Unicode without throwing", () => {
  let row = newQueryParameter();
  row = editQueryParameter(row, "name", "a&b=\uD800");
  row = editQueryParameter(row, "value", "line\nnext\t🌍");
  assert.equal(
    formatQueryUrl({ prefix: "/path", fragment: "#keep", hasQuery: false, rows: [row] }),
    "/path?a%26b%3D%EF%BF%BD=line%0Anext%09%F0%9F%8C%8D#keep",
  );
});
