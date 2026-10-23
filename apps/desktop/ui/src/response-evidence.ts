import type { SessionDetail } from "./models.js";
import type { TimingRow } from "./timings.js";

export function responseEvidence(detail: SessionDetail): TimingRow[] {
  const match = detail.autoResponse,
    selected = match?.snapshot;
  if (!match || !selected) {
    return [];
  }
  const captured = detail.storedBodies.find((body) => body.boundary === "client-response");
  const complete = captured?.availability === "complete";
  const same =
    complete &&
    captured.sha256 === selected.sha256 &&
    captured.retainedBytes === selected.bodyBytes;
  const outcome = complete
    ? !captured.sha256
      ? "Complete response bytes are retained. Compare this entry to compute its body hash; the external file will not be read."
      : same
        ? "Captured response bytes match the selected file snapshot."
        : "Captured response bytes differ from the selected snapshot. Later response processing or HTTP no-body rules may apply."
    : captured?.availability === "capturing"
      ? "Response capture is still in progress."
      : "Complete response bytes are unavailable. Check body retention limits or record another request with response bodies enabled. Earlier bytes will never be recovered from the current file.";
  return [
    {
      id: "rule",
      label: "Matched rule",
      value: match.ruleName + " · Revision " + match.ruleRevision,
      note: "Response " + match.assetReference + "; captured at request time.",
    },
    {
      id: "selected",
      label: "Selected file snapshot",
      value:
        selected.bodyBytes.toLocaleString() +
        " bytes · " +
        (selected.contentCodings.join(", ") || "identity"),
      note: "Frozen content-encoded entity before later response processing; excludes HTTP transfer framing.",
    },
    { id: "selected-hash", label: "Selected SHA-256", value: selected.sha256, note: "" },
    {
      id: "captured",
      label: "Client response evidence",
      value: captured
        ? captured.availability.replaceAll("-", " ") +
          " · " +
          captured.retainedBytes.toLocaleString() +
          " retained bytes"
        : "Unavailable",
      note: outcome + (captured?.reason ? " " + captured.reason : ""),
    },
    {
      id: "captured-hash",
      label: "Captured SHA-256",
      value: complete ? (captured.sha256 ?? "Not computed") : "Unavailable",
      note: complete
        ? "Content-encoded entity bytes retained for inspection and comparison. This does not confirm receipt by the client."
        : "",
    },
    {
      id: "reference",
      label: "Historical response",
      value: detail.id + " / client-response",
      note: "Inspect or compare this Traffic entry using its retained bytes. Save TMCap to preserve those bytes and this snapshot identity; the external path is not exported.",
    },
  ];
}
