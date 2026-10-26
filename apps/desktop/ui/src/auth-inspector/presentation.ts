import type { AuthField, AuthFinding, AuthSource } from "./models.js";

export interface AuthArtifact {
  id: string;
  format: string;
  context: string;
  layers: AuthFinding[];
  readableFields: AuthField[];
  children: AuthArtifact[];
  sources: Array<{ id: string; source: AuthSource }>;
  notices: Array<{ id: string; label: string; diagnostics: string[] }>;
}
const envelopes = new Set([
  "Bearer",
  "Negotiate",
  "NTLM",
  "GSS-API / SPNEGO",
  "SPNEGO",
  "SAML 2.0",
  "Auth parameter",
]);
// Transform descriptions may differ, but a different retained range is a
// different source. Never combine protocol siblings or aggregate OAuth fields.
const sourceKey = (source: AuthSource): string =>
  JSON.stringify([
    source.kind,
    source.label,
    source.occurrence,
    source.headerIndex ?? null,
    source.offset ?? null,
    source.length ?? null,
  ]);
export function authArtifact(root: AuthFinding): AuthArtifact {
  const layers = [root];
  let leaf = root;
  while (
    (leaf.envelope ?? envelopes.has(leaf.format)) &&
    leaf.children.length === 1 &&
    sourceKey(leaf.source) === sourceKey(leaf.children[0]!.source)
  ) {
    leaf = leaf.children[0]!;
    layers.push(leaf);
  }
  const contexts = [
    ...new Set(
      layers
        .slice(0, -1)
        .map((layer) => layer.format)
        .filter((format) => format !== leaf.format),
    ),
  ];
  const readableFields = layers
    .slice()
    .reverse()
    .flatMap((layer) => layer.readableFields ?? []);
  if (leaf.format === "JWT / JWS") {
    // Claims answer the inspection task first. Retain captured order within
    // both groups; copying still traverses the untouched evidence tree.
    const metadata = (field: AuthField): number =>
      Number(field.name.startsWith("JOSE.") || field.name === "Signature length");
    readableFields.sort((a, b) => metadata(a) - metadata(b));
  }
  return {
    id: root.id,
    format: leaf.format,
    context: [...new Set([leaf.role, ...contexts, root === leaf ? "" : root.role])]
      .filter(Boolean)
      .join(" · "),
    layers,
    readableFields,
    children: leaf.children.map(authArtifact),
    sources: [
      ...new Map(
        layers.map((layer) => [sourceKey(layer.source), { id: layer.id, source: layer.source }]),
      ).values(),
    ],
    notices: layers
      .filter(
        (layer) =>
          layer.diagnostics.length ||
          !["recognized", undefined].includes(layer.recognition) ||
          !["decoded", ...(layer === leaf ? [] : ["opaque"])].includes(layer.status),
      )
      .map((layer) => ({
        id: layer.id,
        label: [
          layer.format,
          layer.recognition === "recognized" ? "" : layer.recognition,
          layer.status,
        ]
          .filter(Boolean)
          .join(" · "),
        diagnostics: layer.diagnostics,
      })),
  };
}
