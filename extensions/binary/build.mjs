import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "../../apps/desktop/ui/node_modules/esbuild/lib/main.js";

const root = fileURLToPath(new URL(".", import.meta.url));
const compiler = fileURLToPath(
  new URL("../../apps/desktop/ui/node_modules/typescript/bin/tsc", import.meta.url),
);
execFileSync(process.execPath, [compiler, "--project", root], {
  cwd: root,
  stdio: "inherit",
  windowsHide: true,
});
const result = await esbuild.build({
  absWorkingDir: root,
  entryPoints: Object.fromEntries(
    ["protobuf", "bond", "msgpack", "cbor", "bson"].map((name) => [
      name,
      `typescript/${name}-entry.ts`,
    ]),
  ),
  outdir: "dist/runtime",
  bundle: true,
  format: "esm",
  target: "es2022",
  charset: "ascii",
  legalComments: "none",
  write: false,
});

async function publish(path, bytes) {
  try {
    if ((await readFile(path)).equals(bytes)) return;
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  await mkdir(dirname(path), { recursive: true });
  const temporary = `${path}.${randomUUID()}.tmp`;
  await writeFile(temporary, bytes);
  try {
    for (let attempt = 0; ; attempt++) {
      try {
        await rename(temporary, path);
        break;
      } catch (error) {
        if (attempt >= 5 || !["EACCES", "EPERM", "EBUSY"].includes(error.code)) throw error;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
    }
  } finally {
    await unlink(temporary).catch((error) => {
      if (error.code !== "ENOENT") throw error;
    });
  }
}

const manifest = JSON.parse(await readFile(resolve(root, "manifest.json"), "utf8"));
const outputs = new Map(result.outputFiles.map((file) => [file.path, Buffer.from(file.contents)]));
for (const file of manifest.files) {
  const path = resolve(root, file.path);
  const bytes = outputs.get(path) ?? (await readFile(path));
  file.bytes = bytes.length;
  file.sha256 = createHash("sha256").update(bytes).digest("hex");
}
for (const [path, bytes] of outputs) await publish(path, bytes);
await publish(
  resolve(root, "dist/package-manifest.json"),
  Buffer.from(JSON.stringify(manifest, null, 2) + "\n"),
);
process.stdout.write("Built bundled binary format visualizers.\n");
