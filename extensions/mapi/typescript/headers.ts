import type { DocumentStep, VisualizationInput } from "./host.js";
import { header, latin1, notice } from "./shared.js";

export function visualize(input: VisualizationInput): DocumentStep {
  const message = input[input.direction];
  if (!message) {
    return notice("MAPI/HTTP headers", "unavailable", "This message has no retained headers.");
  }
  const operation = header(input.request, "x-requesttype");
  const limited = message.headers.some((field) => (field.value?.length ?? 0) > 16_384);
  return {
    kind: "document",
    document: {
      title: "MAPI/HTTP headers",
      updates: "final",
      status: limited ? "limited" : operation ? "complete" : "partial",
      presentation: "tree",
      summary: limited
        ? "Some header values exceed the display limit. Their complete retained values remain available in Raw headers."
        : operation
          ? `Operation: ${operation.slice(0, 256)}`
          : "The request operation is unavailable or redacted.",
      roots: message.headers.map((field, index) => ({
        id: `h.${index}`,
        name: field.name,
        value: {
          kind: "string",
          value: field.redacted
            ? "Redacted"
            : field.value
              ? latin1(field.value).slice(0, 16_384)
              : "Unavailable",
        },
        format: { protocol: "MAPI/HTTP header" },
        children: [],
      })),
    },
  };
}
