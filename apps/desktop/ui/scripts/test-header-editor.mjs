import assert from "node:assert/strict";
import { test } from "node:test";
import {
  formatHeaderText,
  HeaderValidationError,
  parseHeaderText,
  validateHeaderFields,
} from "../src/header-editor/headers.ts";

test("header drafts preserve order, case, duplicate names, empty values and colon values", () => {
  const headers = [
    { name: "X-Custom!", value: "one: two" },
    { name: "x-custom!", value: "second" },
    { name: "X-Empty", value: "" },
    { name: "X-Whitespace", value: "  preserved \t" },
  ];
  assert.deepEqual(parseHeaderText(formatHeaderText(headers)), headers);
  assert.deepEqual(
    parseHeaderText("X-Custom!: one: two\r\nx-custom!: second\r\nX-Empty:\r\n"),
    headers.slice(0, 3),
  );
});

test("raw parsing identifies the original line and never drops malformed lines", () => {
  for (const [line, field] of [
    ["missing colon", "name"],
    [": missing name", "name"],
    ["Bad Name: value", "name"],
    [" Name: value", "name"],
    ["X-Test: before\u000bafter", "value"],
    ["X-Test: \u000b", "value"],
    ["X-Test: before\rafter", "value"],
  ]) {
    assert.throws(
      () => parseHeaderText(`X-Good: yes\n\n${line}\nX-End: keep`),
      (error) => {
        assert.ok(error instanceof HeaderValidationError);
        assert.equal(error.line, 3);
        assert.equal(error.field, field);
        assert.match(error.message, /^Line 3:/);
        return true;
      },
    );
  }
  assert.deepEqual(parseHeaderText(" \t\nX-Valid: yes\n\t"), [{ name: "X-Valid", value: "yes" }]);
});

test("structured validation rejects a blank row and value newlines without filtering", () => {
  assert.throws(() => validateHeaderFields([{ name: "", value: "" }]), /enter a name or remove/);
  for (const value of ["a\nb", "a\rb", "a\0b", "a\u007fb"]) {
    assert.throws(
      () => validateHeaderFields([{ name: "X-Test", value }]),
      (error) => {
        assert.equal(error.field, "value");
        return true;
      },
    );
  }
  validateHeaderFields([
    { name: "X-Tab", value: "a\tb" },
    { name: "X-Unicode", value: "café 🌍" },
  ]);
});
