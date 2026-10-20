import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { watch } from "node:fs";
import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "../../apps/desktop/ui/node_modules/esbuild/lib/main.js";

// Build time only. Installed extensions run the resulting ordinary ES2022.
const root = fileURLToPath(new URL(".", import.meta.url));
const compiler = fileURLToPath(
  new URL("../../apps/desktop/ui/node_modules/typescript/bin/tsc", import.meta.url),
);

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
        // Active development snapshots briefly hold files without delete sharing.
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

async function build() {
  // Check types before publishing any replacement runtime files.
  execFileSync(process.execPath, [compiler, "--project", root], {
    cwd: root,
    stdio: "inherit",
    windowsHide: true,
  });
  const result = await esbuild.build({
    absWorkingDir: root,
    entryPoints: { body: "typescript/body.ts", headers: "typescript/headers.ts" },
    outdir: "dist/runtime",
    bundle: true,
    format: "iife",
    globalName: "mapiImplementation",
    target: "es2022",
    minify: true,
    charset: "ascii",
    sourcemap: false,
    legalComments: "none",
    metafile: true,
    write: false,
  });
  const headers = result.metafile.outputs["dist/runtime/headers.js"];
  if (
    !headers ||
    Object.keys(headers.inputs).some(
      (name) => !["typescript/headers.ts", "typescript/shared.ts"].includes(name),
    )
  ) {
    throw new Error("The header contribution acquired a binary-parser dependency");
  }
  // API 1 requires explicit synchronous declarations. Keep those stable while
  // allowing the implementation's private names to be minified by the bundler.
  const outputs = new Map(
    result.outputFiles.map((file) => {
      const path = relative(root, file.path).replaceAll("\\", "/");
      const handlers = path.endsWith("/headers.js")
        ? ["visualize"]
        : ["visualize", "resume", "resumeContext", "resumeDecoded", "expand", "onTraffic"];
      const wrappers = handlers
        .map(
          (name) => `export function ${name}(...args){return mapiImplementation.${name}(...args);}`,
        )
        .join("\n");
      return [path, Buffer.from(file.text + "\n" + wrappers + "\n")];
    }),
  );
  const manifest = JSON.parse(await readFile(resolve(root, "manifest.json"), "utf8"));
  const declared = new Set(manifest.files.map((file) => file.path));
  for (const path of outputs.keys()) {
    if (!declared.has(path)) throw new Error(`Undeclared runtime output: ${path}`);
  }
  for (const file of manifest.files) {
    const path = resolve(root, file.path);
    const name = relative(root, path);
    if (name.startsWith("..") || name.includes(":"))
      throw new Error("Manifest path escaped the extension");
    const bytes = outputs.get(file.path) ?? (await readFile(path));
    file.bytes = bytes.length;
    file.sha256 = createHash("sha256").update(bytes).digest("hex");
  }
  for (const [path, bytes] of outputs) await publish(resolve(root, path), bytes);
  await publish(
    resolve(root, "dist/package-manifest.json"),
    Buffer.from(JSON.stringify(manifest, null, 2) + "\n"),
  );

  if (process.argv.includes("--development")) {
    const development = structuredClone(manifest);
    development.id = "com.transmog.mapi.development";
    development.name = "MAPI/HTTP (Development)";
    const folder = resolve(root, "dist/development");
    for (const file of development.files) {
      await publish(
        resolve(folder, file.path),
        outputs.get(file.path) ?? (await readFile(resolve(root, file.path))),
      );
    }
    await publish(
      resolve(folder, "manifest.json"),
      Buffer.from(JSON.stringify(development, null, 2) + "\n"),
    );
    process.stdout.write("Development folder: " + folder + "\n");
  }

  // The reference bridge is a separate developer artifact, never in manifest.files.
  const testing = await esbuild.build({
    absWorkingDir: root,
    entryPoints: ["typescript/testing.ts"],
    outfile: "dist/testing.bundle.js",
    bundle: true,
    format: "esm",
    target: "es2022",
    minify: true,
    write: false,
    legalComments: "none",
  });
  await publish(
    resolve(root, "dist/testing.bundle.js"),
    Buffer.from(testing.outputFiles[0].contents),
  );
  process.stdout.write(
    `MAPI ES2022: ${manifest.files.length} declared payload files, ${manifest.files.reduce((total, file) => total + file.bytes, 0).toLocaleString("en-US")} bytes\n`,
  );
}

await build();
if (process.argv.includes("--watch")) {
  let timer;
  let pending = false;
  let running = false;
  async function rebuild() {
    pending = true;
    if (running) return;
    running = true;
    try {
      while (pending) {
        pending = false;
        try {
          await build();
        } catch (error) {
          process.stderr.write(`${error.message}\n`);
        }
      }
    } finally {
      running = false;
    }
  }
  watch(root, { recursive: true }, (_event, filename) => {
    const path = String(filename ?? "").replaceAll("\\", "/");
    if (path.startsWith("dist/") || path.startsWith("test/") || path.startsWith("build/")) return;
    if (
      !path.startsWith("typescript/") &&
      !["manifest.json", "tsconfig.json", "README.runtime.md", "LICENSES/Office-MIT.txt"].includes(
        path,
      )
    )
      return;
    clearTimeout(timer);
    timer = setTimeout(rebuild, 200);
  });
  process.stdout.write(
    "Watching MAPI authoring inputs. Enable hot reload in Extensions settings to replace active instances.\n",
  );
}
