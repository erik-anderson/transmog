import { copyFile, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { bundleClient, prepareBuild } from "@microsoft/webui/build.js";
import { buildCredits } from "./scripts/build-credits.mjs";
import { monacoCompletionLifecycle } from "./scripts/monaco-completion-lifecycle.mjs";

const workingDirectory = fileURLToPath(new URL("./", import.meta.url));
const projectDirectory = resolve(workingDirectory, "../../..");
const outputDirectory = resolve(workingDirectory, "dist");
const templateOutput = join(outputDirectory, "webui");

async function installedVersion(name) {
  let directory = dirname(fileURLToPath(import.meta.resolve(name)));
  for (;;) {
    try {
      const metadata = JSON.parse(await readFile(join(directory, "package.json"), "utf8"));
      if (metadata.name === name && typeof metadata.version === "string") return metadata.version;
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    const parent = dirname(directory);
    if (parent === directory) throw new Error("Missing installed package metadata: " + name);
    directory = parent;
  }
}
for (const file of await readdir(join(workingDirectory, "src"), { recursive: true })) {
  if (!file.endsWith(".html")) continue;
  const template = await readFile(join(workingDirectory, "src", file), "utf8");
  if (template.includes("(event)")) {
    throw new Error(
      "WebUI event handlers must pass the reserved event token `e`, not `event`: " + file,
    );
  }
}

// Discard stale hashed chunks before generating this build's asset inventory.
if (relative(workingDirectory, outputDirectory) !== "dist")
  throw new Error("Unsafe build output directory: " + outputDirectory);
await rm(outputDirectory, { recursive: true, force: true });
await mkdir(outputDirectory, { recursive: true });
const templates = prepareBuild({
  appDir: join(workingDirectory, "src"),
  plugin: "webui",
  css: "link",
  assetBase: "/",
  cssFileNameTemplate: "[name]-[hash].[ext]",
  modulesOut: join(templateOutput, "modules"),
});
templates.writeModules();
const client = await bundleClient({
  // The UI imports the shared traffic filter catalog from the Rust workspace.
  project: projectDirectory,
  entries: { app: relative(projectDirectory, join(workingDirectory, "src/index.ts")) },
  outDir: outputDirectory,
  tsconfig: relative(projectDirectory, join(workingDirectory, "tsconfig.json")),
  chunkNames: "chunks/[name]-[hash].js",
  external: ["/monaco.js"],
  minify: true,
  target: "es2022",
});
for (const { code, message } of client.warnings) console.warn(`${code}: ${message}`);
const compiled = templates.finish({ projectionManifests: [client.manifestPath] });
if (compiled.warnings.length) throw new Error(compiled.warnings.join("\n"));
// The browser fixtures use the same preparation whose generated modules entered the bundle.
await writeFile(join(templateOutput, "protocol.bin"), compiled.protocol);
await writeFile(join(templateOutput, "stats.json"), JSON.stringify(compiled.stats) + "\n");
for (let index = 0; index < compiled.cssFiles.length; index += 2)
  await writeFile(join(templateOutput, compiled.cssFiles[index]), compiled.cssFiles[index + 1]);
for (const pack of compiled.lazyPackFiles) {
  const file = join(templateOutput, pack.path);
  await mkdir(dirname(file), { recursive: true });
  await writeFile(file, pack.content);
}

// Monaco has its own stylesheet, workers, and completion lifecycle patch.
const monaco = await esbuild.build({
  absWorkingDir: workingDirectory,
  entryPoints: { monaco: "src/monaco.ts" },
  outdir: outputDirectory,
  chunkNames: "chunks/[name]-[hash]",
  bundle: true,
  splitting: true,
  metafile: true,
  format: "esm",
  minify: true,
  sourcemap: false,
  target: ["es2022"],
  legalComments: "inline",
  loader: { ".ttf": "dataurl" },
  plugins: [monacoCompletionLifecycle()],
});

const clientOutputs = Object.fromEntries(
  Object.entries(client.metafile.outputs).map(([path, metadata]) => [
    relative(workingDirectory, resolve(projectDirectory, path)).replaceAll("\\", "/"),
    metadata,
  ]),
);
const browserOutputs = { ...clientOutputs, ...monaco.metafile.outputs };
const browserInputs = { ...client.metafile.inputs, ...monaco.metafile.inputs };
const outputs = { ...browserOutputs };
const inputs = { ...browserInputs };
for (const [entryPoint, outfile] of [
  ["node_modules/monaco-editor/esm/vs/editor/editor.worker.js", "monaco-editor.worker.js"],
  ["script-worker.mjs", "monaco-ts.worker.js"],
  ["node_modules/monaco-editor/esm/vs/language/json/json.worker.js", "monaco-json.worker.js"],
  ["node_modules/monaco-editor/esm/vs/language/css/css.worker.js", "monaco-css.worker.js"],
  ["node_modules/monaco-editor/esm/vs/language/html/html.worker.js", "monaco-html.worker.js"],
]) {
  const worker = await esbuild.build({
    absWorkingDir: workingDirectory,
    entryPoints: [join(workingDirectory, entryPoint)],
    outfile: join(outputDirectory, outfile),
    bundle: true,
    format: "esm",
    minify: true,
    sourcemap: false,
    target: ["es2022"],
    legalComments: "inline",
    metafile: true,
  });
  Object.assign(outputs, worker.metafile.outputs);
  Object.assign(inputs, worker.metafile.inputs);
}

// Document-scoped fonts and styles ship outside the client asset inventory.
const documentStyles = await esbuild.build({
  absWorkingDir: workingDirectory,
  entryPoints: ["src/document.css"],
  outfile: join(outputDirectory, "document.css"),
  bundle: true,
  minify: true,
  loader: { ".ttf": "dataurl" },
  metafile: true,
});
await buildCredits(workingDirectory, join(outputDirectory, "credits.json"), {
  outputs: { ...outputs, ...documentStyles.metafile.outputs },
  inputs: { ...inputs, ...documentStyles.metafile.inputs },
});

// The native origin embeds only the current bundler outputs, including dynamic chunks.
const assets = Object.entries(outputs)
  .map(([file, metadata]) => {
    const name = relative(outputDirectory, resolve(workingDirectory, file)).replaceAll("\\", "/");
    if (!/^(?:chunks\/)?[a-zA-Z0-9_.-]+\.(?:js|css)$/.test(name))
      throw new Error("Unsafe client asset path: " + name);
    return {
      path: "/" + name,
      file: name,
      contentType: name.endsWith(".css")
        ? "text/css; charset=utf-8"
        : "text/javascript; charset=utf-8",
      bytes: metadata.bytes,
    };
  })
  .sort((a, b) => a.path.localeCompare(b.path));
assets.push({
  path: "/credits.json",
  file: "credits.json",
  contentType: "application/json; charset=utf-8",
  bytes: (await readFile(join(outputDirectory, "credits.json"))).length,
});
await writeFile(
  join(outputDirectory, "client-assets.json"),
  JSON.stringify(assets, null, 2) + "\n",
);
await writeFile(
  join(outputDirectory, "client-metafile.json"),
  JSON.stringify({ inputs: browserInputs, outputs: browserOutputs }, null, 2) + "\n",
);
// Bind frontend versions to this exact set of bundled assets for native diagnostics.
await writeFile(
  join(outputDirectory, "client-build-info.json"),
  JSON.stringify(
    {
      dependencies: Object.fromEntries(
        await Promise.all(
          ["@microsoft/webui-framework", "@tauri-apps/api"].map(async (name) => [
            name,
            await installedVersion(name),
          ]),
        ),
      ),
      assets: await Promise.all(
        assets.map(async (asset) => ({
          file: asset.file,
          sha256: createHash("sha256")
            .update(await readFile(join(outputDirectory, asset.file)))
            .digest("hex"),
        })),
      ),
    },
    null,
    2,
  ) + "\n",
);
await copyFile(
  join(workingDirectory, "../icons/icon.svg"),
  join(outputDirectory, "transmog-icon.svg"),
);
