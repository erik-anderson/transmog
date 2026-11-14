import type { ParsedField } from "./model.js";
import type { Diagnostic, Value, ParseStatus } from "transmog:extensions/v1";
import { maximumDisplayStringLength } from "./shared.js";
import { displayLimits } from "./view-limits.js";
import { isInputLimit } from "./input-diagnostics.js";

type ClipText = (value: string, maximumCodeUnits: number, maximumUtf8Bytes?: number) => string;

export function fieldValue(field: ParsedField, boundedText: ClipText): Value | undefined {
  const parsed = field.value;
  if (!parsed) {
    return field.children.length ? { kind: "object" } : undefined;
  }
  switch (parsed.kind) {
    case "string":
      return {
        kind: "string",
        value: boundedText(
          parsed.value,
          maximumDisplayStringLength,
          maximumDisplayStringLength * displayLimits.utf8BytesPerCodeUnit,
        ),
      };
    case "integer":
      return { kind: "integer", value: parsed.value };
    case "guid":
      return { kind: "guid", value: parsed.value };
    case "boolean":
      return { kind: "boolean", value: parsed.value };
    case "bytes":
      return { kind: "bytes" };
    case "float":
      return typeof parsed.value === "number"
        ? { kind: "float", value: parsed.value }
        : { kind: "string", value: parsed.value };
    case "enum":
      return parsed.flags
        ? {
            kind: "flags",
            value: {
              names: parsed.names
                .slice(0, displayLimits.flagNames)
                .map((name) => boundedText(name, displayLimits.enumName)),
              raw: parsed.value,
            },
          }
        : {
            kind: "enum",
            value: {
              name: boundedText(parsed.names[0] ?? parsed.value, displayLimits.enumName),
              raw: parsed.value,
            },
          };
  }
}
export function parseOutcome(
  root: ParsedField,
  diagnostics: readonly Diagnostic[],
  boundedText: ClipText,
) {
  let diagnosticsClipped = diagnostics.length > displayLimits.diagnostics;
  let limited = diagnostics.some(isInputLimit);
  let malformed = false;
  let partial = diagnostics.length !== 0;
  const incompleteInput = diagnostics.some((item) => item.code === "body-incomplete");
  const issues: Diagnostic[] = diagnostics.slice(0, displayLimits.diagnostics).map((item) => ({
    code: boundedText(item.code, displayLimits.diagnosticCode),
    message: boundedText(item.message, displayLimits.diagnosticMessage),
  }));
  // Compare original messages, so clipping cannot turn one issue into duplicates.
  const seenMessages = new Set(diagnostics.map((item) => item.message));
  const inspect = (field: ParsedField): void => {
    if (field.issue) {
      limited ||= field.issue.kind === "limit";
      malformed ||= field.issue.kind === "malformed";
      partial ||= field.issue.kind === "missing-context" || field.issue.kind === "unsupported";
      if (!seenMessages.has(field.issue.message)) {
        seenMessages.add(field.issue.message);
        if (issues.length >= displayLimits.diagnostics) {
          diagnosticsClipped = true;
        } else {
          issues.push({
            code: `mapi-${field.issue.kind}`,
            message: boundedText(field.issue.message, displayLimits.diagnosticMessage),
          });
        }
      }
    }
    if (field.name === "RopBufferTooSmall response") {
      partial = true;
    }
    field.children.forEach(inspect);
  };
  inspect(root);

  return {
    limited: limited || diagnosticsClipped,
    diagnosticsClipped,
    malformed,
    partial,
    incompleteInput,
    issues,
  };
}

export function parseStatus(outcome: {
  limited: boolean;
  malformed: boolean;
  partial: boolean;
  incompleteInput: boolean;
}): ParseStatus {
  if (outcome.limited) {
    return "limited";
  }
  if (outcome.malformed && !outcome.incompleteInput) {
    return "malformed";
  }
  return outcome.partial || outcome.malformed ? "partial" : "complete";
}
