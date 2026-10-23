import type { BodyEditReport } from "./models.js";
export function bodyEditSummary(report: BodyEditReport): string {
  const labels: Record<string, string> = {
    applied: "Applied",
    "not-found": "Not applied: text not found",
    unchanged: "No decoded byte change",
    "count-mismatch": "Failed: required match count was not met",
    "invalid-text": "Failed: invalid UTF-8 text",
    "invalid-encoding": "Failed: unsupported or ambiguous character encoding",
    "partial-response": "Failed: partial response",
    "input-limit": "Failed: input bound exceeded",
    "output-limit": "Failed: output bound exceeded",
  };
  return (
    (labels[report.outcome] ?? "Outcome unavailable") +
    (report.matches === null
      ? ""
      : ` · ${report.matches} matches · ${report.replacements} replacements`) +
    (report.beforeBytes === null ? "" : ` · ${report.beforeBytes} decoded input bytes`) +
    (report.afterBytes === null ? "" : ` → ${report.afterBytes} output bytes`)
  );
}
