import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
const output = await build({
  entryPoints: [
    new URL("../src/auth-inspector/presentation.ts", import.meta.url).pathname.replace(
      /^\/([A-Za-z]:)/,
      "$1",
    ),
  ],
  bundle: true,
  platform: "node",
  format: "esm",
  write: false,
});
const { authArtifact } = await import(
  "data:text/javascript;base64," + Buffer.from(output.outputFiles[0].text).toString("base64")
);
const source = {
  kind: "header",
  label: "Authorization",
  occurrence: 0,
  headerIndex: 0,
  offset: 0,
  length: 100,
  transforms: [],
};
const field = { id: "issuer", name: "iss", value: "issuer", available: true, sensitive: false };
const node = (id, format, children = [], extra = {}) => ({
  id,
  format,
  role: "credential",
  source,
  recognition: "recognized",
  status: "decoded",
  verification: "Not checked",
  fields: [],
  readableFields: [],
  technicalFields: [],
  children,
  diagnostics: [],
  ...extra,
});
test("same-source envelope chains show claims first while preserving original evidence and warning ownership", () => {
  const jwt = node("jwt", "JWT / JWS", [], {
    readableFields: [field],
    source: { ...source, transforms: ["Base64url"] },
  });
  const root = node("bearer", "Bearer", [jwt], {
    status: "malformed",
    diagnostics: ["Truncated credential"],
  });
  const before = structuredClone(root),
    artifact = authArtifact(root);
  assert.equal(artifact.format, "JWT / JWS");
  assert.deepEqual(artifact.readableFields, [field]);
  assert.equal(artifact.sources.length, 1);
  assert.deepEqual(artifact.layers, [root, jwt]);
  assert.deepEqual(artifact.notices, [
    { id: "bearer", label: "Bearer · malformed", diagnostics: ["Truncated credential"] },
  ]);
  assert.deepEqual(root, before);
});
test("sibling challenges and different retained ranges remain separate", () => {
  const jwt = node("jwt", "JWT / JWS");
  assert.equal(
    authArtifact(node("bearer", "Bearer", [jwt, node("jwe", "JWE")])).children.length,
    2,
  );
  for (const change of [
    { offset: 5 },
    { headerIndex: 1 },
    { occurrence: 1 },
    { kind: "body" },
    { label: "Other source" },
    { length: 50 },
  ]) {
    const artifact = authArtifact(
      node("bearer", "Bearer", [{ ...jwt, source: { ...source, ...change } }]),
    );
    assert.equal(artifact.format, "Bearer");
    assert.equal(artifact.children.length, 1);
  }
  assert.equal(authArtifact(node("oauth", "OAuth / OpenID Connect", [jwt])).children.length, 1);
});
test("native envelope metadata overrides legacy format presentation", () => {
  const child = node("child", "JWT / JWS");
  const retained = authArtifact(node("root", "Bearer", [child], { envelope: false }));
  assert.equal(retained.format, "Bearer");
  assert.equal(retained.children.length, 1);
  const promoted = authArtifact(node("root", "Native wrapper", [child], { envelope: true }));
  assert.equal(promoted.format, "JWT / JWS");
  assert.equal(promoted.layers.length, 2);
});
test("JWT claims precede JOSE metadata without changing captured field order", () => {
  const fields = [
    { ...field, id: "alg", name: "JOSE.alg" },
    { ...field, id: "sig", name: "Signature length" },
    field,
    { ...field, id: "aud", name: "aud[0]" },
  ];
  const jwt = node("jwt", "JWT / JWS", [], { fields, readableFields: fields });
  assert.deepEqual(
    authArtifact(jwt).readableFields.map((field) => field.id),
    ["issuer", "aud", "alg", "sig"],
  );
  assert.deepEqual(
    jwt.fields.map((field) => field.id),
    ["alg", "sig", "issuer", "aud"],
  );
});
test("nested negotiation retains mechanism role, encrypted state, redaction and every decoder layer", () => {
  const unavailable = {
    ...field,
    id: "secret",
    name: "Ticket",
    available: false,
    sensitive: true,
    value: "[unavailable]",
  };
  const kerberos = node("k", "Kerberos V5", [], {
    role: "AP-REP",
    status: "encrypted",
    readableFields: [unavailable],
  });
  const artifact = authArtifact(
    node("n", "Negotiate", [node("s", "GSS-API / SPNEGO", [kerberos])], { status: "opaque" }),
  );
  assert.equal(artifact.layers.length, 3);
  assert.match(artifact.context, /AP-REP/);
  assert.equal(artifact.notices[0].label, "Kerberos V5 · encrypted");
  assert.deepEqual(artifact.readableFields, [unavailable]);
});
