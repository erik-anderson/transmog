import { readFile, readdir, realpath, writeFile } from "node:fs/promises";
import { dirname, relative, resolve, join } from "node:path";
import { execFileSync } from "node:child_process";

const shippedPackages = [
  "transmog-desktop",
  "transmog",
  "transmog-script-host",
  "transmog-preview-worker",
];

export function shippedCargoPackages(metadata, tree) {
  // Cargo evaluates target conditions and features. Its normal-edge tree also
  // excludes development/build dependencies and procedural-macro executables.
  const identities = new Set();
  const names = new Set();
  for (const line of tree.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const match = /^([\w-]+) v([^ ]+)(?: |$)/.exec(line);
    if (!match) throw new Error("Invalid shipped Cargo dependency: " + line);
    names.add(match[1]);
    identities.add(match[1] + "@" + match[2]);
  }
  for (const name of shippedPackages)
    if (!names.has(name)) throw new Error("Missing shipped Cargo root: " + name);
  const packages = metadata.packages.filter((pkg) => identities.has(pkg.name + "@" + pkg.version));
  for (const identity of identities)
    if (packages.filter((pkg) => pkg.name + "@" + pkg.version === identity).length !== 1)
      throw new Error("Missing or ambiguous shipped Cargo package: " + identity);
  return packages;
}

export function creditsTarget(explicit, rustVersion) {
  const target = explicit || /^host: (.+)$/m.exec(rustVersion)?.[1]?.trim();
  if (!target || !/^[\w]+(?:-[\w]+)+$/.test(target))
    throw new Error("Credits require a Rust target triple");
  return target;
}

export function bundledNpmPackages(uiRoot, outputs, inputs = {}) {
  if (!outputs) throw new Error("Credits require the shipped browser bundle inputs");
  const usedInputs = new Set();
  for (const output of Object.values(outputs)) {
    for (const [input, contribution] of Object.entries(output.inputs)) {
      if (contribution.bytesInOutput) usedInputs.add(input);
    }
  }
  // Inlined CSS assets are charged to their stylesheet, not to the font/image
  // input. Follow its local URL imports so those shipped bytes keep notices.
  for (const input of usedInputs)
    for (const dependency of inputs[input]?.imports ?? [])
      if (dependency.kind === "url-token" && !dependency.external) usedInputs.add(dependency.path);
  const packages = new Set();
  for (const input of usedInputs) {
    const path = relative(uiRoot, resolve(uiRoot, input)).replaceAll("\\", "/");
    if (!path.includes("node_modules/")) continue;
    if (!path.startsWith("node_modules/"))
      throw new Error("Bundled package is outside the UI lockfile: " + input);
    const start = path.lastIndexOf("node_modules/") + "node_modules/".length;
    const parts = path.slice(start).split("/");
    packages.add(path.slice(0, start) + parts.slice(0, parts[0].startsWith("@") ? 2 : 1).join("/"));
  }
  return [...packages].sort();
}

export async function npmCredits(uiRoot, outputs, inputs) {
  const lock = JSON.parse(await readFile(join(uiRoot, "package-lock.json"), "utf8"));
  const entries = new Map();
  for (const path of bundledNpmPackages(uiRoot, outputs, inputs)) {
    const pkg = lock.packages[path];
    if (!pkg?.version) throw new Error("Bundled package is missing from the UI lockfile: " + path);
    const name = path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
    const files = await notices(join(uiRoot, path));
    if (!files.length) throw new Error("Missing bundled license text for " + name);
    // A development dependency can contribute shipped helper code. Credit the
    // actual bundle inputs, independently of the dependency's dev flag.
    const id = name + "@" + pkg.version;
    const license = pkg.license ?? "";
    if (!entries.has(id))
      entries.set(id, { id, name, version: pkg.version, license, texts: new Set() });
    const entry = entries.get(id);
    if (entry.license !== license)
      throw new Error("Conflicting bundled license metadata for " + id);
    for (const file of files) entry.texts.add(file.text);
  }
  return [...entries.values()].map(({ texts, ...entry }) => ({
    ...entry,
    text: [...texts].join("\n\n"),
  }));
}

const noticeName =
  /^(?:licen[sc]e|copying|copyright|notice|unlicense|authors|third.?party.?notices)(?:[._-].*)?$/i;
async function notices(root, depth = 0, vendored = false) {
  const found = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isFile() && noticeName.test(entry.name))
      found.push({ file: path, text: await readFile(path, "utf8") });
    else if (
      entry.isDirectory() &&
      depth < 12 &&
      (vendored ||
        /^(?:vendor|deps|src|third_party|boringssl|quiche|openssl|zlib|v8|licenses)$/i.test(
          entry.name,
        ))
    )
      found.push(
        ...(await notices(
          path,
          depth + 1,
          vendored || /^(?:vendor|third_party|boringssl|v8)$/i.test(entry.name),
        )),
      );
  }
  return found;
}
export async function cargoCredits(repository, metadata) {
  const sources = new Map(
    Object.entries(
      JSON.parse(
        await readFile(join(repository, "third_party/rust-transport/sources.json"), "utf8"),
      ),
    ),
  );
  const packages = [];
  for (const pkg of metadata.packages) {
    const source = sources.get(pkg.name);
    const parser = pkg.name === "swc_ecma_parser";
    const v8 = pkg.name === "v8";
    if (source) {
      const manifest = await realpath(
        resolve(repository, "third_party/rust-transport/vendor", pkg.name, "Cargo.toml"),
      );
      if (
        pkg.source !== null ||
        pkg.version !== source.version ||
        (await realpath(pkg.manifest_path)) !== manifest
      )
        throw new Error("Unreviewed transport dependency: " + pkg.name);
    }
    if (parser) {
      const directory = resolve(repository, "third_party/swc-ecma-parser");
      const receipt = JSON.parse(await readFile(join(directory, "transmog-snapshot.json"), "utf8"));
      if (
        pkg.source !== null ||
        pkg.version !== receipt.source.version ||
        (await realpath(pkg.manifest_path)) !== (await realpath(join(directory, "Cargo.toml")))
      )
        throw new Error("Unreviewed SWC parser dependency");
    }
    if (v8) {
      const directory = resolve(repository, "third_party/rusty-v8");
      const pin = JSON.parse(await readFile(join(directory, "source.json"), "utf8"));
      if (
        pkg.source !== null ||
        pkg.version !== pin.version ||
        (await realpath(pkg.manifest_path)) !== (await realpath(join(directory, "Cargo.toml")))
      )
        throw new Error("Unreviewed V8 dependency");
    }
    if (source || parser || v8 || pkg.source) packages.push(pkg);
  }
  const entries = [];
  const byRepository = new Map();
  const repositoryKey = (pkg) => pkg.repository + "\0" + pkg.license;
  for (const pkg of packages) {
    const files = await notices(dirname(pkg.manifest_path));
    if (files.length && pkg.repository) byRepository.set(repositoryKey(pkg), files);
  }
  for (const pkg of packages) {
    let files = await notices(dirname(pkg.manifest_path));
    if (pkg.license_file && !files.some((file) => file.file === pkg.license_file))
      files.push({ file: pkg.license_file, text: await readFile(pkg.license_file, "utf8") });
    if (!files.length && pkg.repository) {
      const key = repositoryKey(pkg);
      if (!byRepository.has(key)) {
        // Some published crates omit their project's notice. A build-only sibling
        // can supply that notice without becoming an application credit itself.
        for (const candidate of metadata.noticeSources ?? packages) {
          if (candidate.repository !== pkg.repository || candidate.license !== pkg.license)
            continue;
          files = await notices(dirname(candidate.manifest_path));
          if (files.length) break;
        }
        byRepository.set(key, files);
      }
      files = byRepository.get(key);
    }
    if (!files.length) {
      const fallback = join(repository, "third-party", pkg.name, pkg.version);
      try {
        const source = JSON.parse(await readFile(join(fallback, "source.json"), "utf8"));
        const vcs = JSON.parse(
          await readFile(join(dirname(pkg.manifest_path), ".cargo_vcs_info.json"), "utf8"),
        );
        if (
          source.version !== pkg.version ||
          source.package !== pkg.name ||
          source.commit !== vcs.git.sha1 ||
          (source.license !== undefined && source.license !== pkg.license)
        )
          throw new Error("Stale license snapshot");
        files = [
          {
            file: join(fallback, "LICENSE"),
            text: await readFile(join(fallback, "LICENSE"), "utf8"),
          },
        ];
      } catch {
        throw new Error(
          "Missing or stale bundled license text for " + pkg.name + " " + pkg.version,
        );
      }
    }
    entries.push({
      id: pkg.name + "@" + pkg.version,
      name: pkg.name,
      version: pkg.version,
      license: pkg.license ?? "",
      text: files.map((file) => file.text).join("\n\n"),
    });
  }
  return entries;
}
export async function extensionCredits(repository) {
  const mapi = JSON.parse(
    await readFile(join(repository, "third_party/mapi-inspector/source.json"), "utf8"),
  );
  return [
    {
      id: "mapi-inspector",
      name: "Microsoft MAPI protocol enum data",
      version: mapi.revision,
      license: "MIT",
      text: await readFile(join(repository, "third_party/mapi-inspector/LICENSE"), "utf8"),
    },
  ];
}
export function shippedCargoMetadata(repository, target) {
  const options = {
    cwd: repository,
    maxBuffer: 32 * 1024 * 1024,
    encoding: "utf8",
    windowsHide: true,
  };
  const metadata = JSON.parse(
    execFileSync(
      "cargo",
      ["metadata", "--format-version", "1", "--locked", "--offline", "--filter-platform", target],
      options,
    ),
  );
  const tree = execFileSync(
    "cargo",
    [
      "tree",
      "--locked",
      "--offline",
      "--target",
      target,
      "--edges",
      "normal,no-proc-macro",
      "--prefix",
      "none",
      ...shippedPackages.flatMap((name) => ["-p", name]),
    ],
    options,
  );
  return {
    packages: shippedCargoPackages(metadata, tree),
    noticeSources: metadata.packages,
  };
}

export async function buildCredits(uiRoot, output, { outputs, inputs }) {
  const repository = resolve(uiRoot, "../../..");
  const explicitTarget = process.env.CARGO_BUILD_TARGET;
  const target = creditsTarget(
    explicitTarget,
    explicitTarget
      ? ""
      : execFileSync("rustc", ["-vV"], {
          cwd: repository,
          encoding: "utf8",
          windowsHide: true,
        }),
  );
  const entries = await cargoCredits(repository, shippedCargoMetadata(repository, target));
  entries.push(...(await extensionCredits(repository)));
  entries.push(...(await npmCredits(uiRoot, outputs, inputs)));
  for (const [directory, name, versionFile, versionField, license] of [
    ["boringssl-pki", "BoringSSL PKI", "manifest.json", "revision", "Apache-2.0"],
    [
      "chromium-suffix-list",
      "Chromium Public Suffix List",
      "snapshot.json",
      "chromiumVersion",
      "MPL-2.0",
    ],
    [
      "chromium-root-store",
      "Chromium root-store data",
      "snapshot.json",
      "rootStoreVersion",
      "BSD-3-Clause",
    ],
    [
      "chromium-certificate-policy",
      "Chromium certificate policy data",
      "snapshot.json",
      "chromiumVersion",
      "BSD-3-Clause",
    ],
  ]) {
    const base = join(repository, "third_party", directory);
    const receipt = JSON.parse(await readFile(join(base, versionFile), "utf8"));
    entries.push({
      id: directory,
      name,
      version: String(receipt[versionField]),
      license,
      text: await readFile(join(base, "LICENSE"), "utf8"),
    });
  }
  entries.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
  await writeFile(
    output,
    JSON.stringify({
      target,
      summary:
        "Transmog includes third-party libraries, native code and visualizer extensions. Their license and copyright notices are available below.",
      entries,
    }),
  );
}
