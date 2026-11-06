import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  bundledNpmPackages,
  cargoCredits,
  creditsTarget,
  extensionCredits,
  npmCredits,
  shippedCargoMetadata,
  shippedCargoPackages,
} from "./build-credits.mjs";

const repository = fileURLToPath(new URL("../../../../", import.meta.url));
const sources = JSON.parse(
  await readFile(resolve(repository, "third_party/rust-transport/sources.json"), "utf8"),
);
const packages = Object.entries(sources).map(([name, source]) => ({
  id: name,
  name,
  version: source.version,
  source: null,
  license: name === "quiche" ? "BSD-2-Clause" : "MIT",
  manifest_path: resolve(repository, "third_party/rust-transport/vendor", name, "Cargo.toml"),
}));

test("credits select the explicit release target or the Rust host", () => {
  assert.equal(
    creditsTarget("aarch64-pc-windows-msvc", "host: x86_64-pc-windows-msvc"),
    "aarch64-pc-windows-msvc",
  );
  assert.equal(
    creditsTarget(undefined, "rustc 1.99.1\nhost: x86_64-pc-windows-msvc\n"),
    "x86_64-pc-windows-msvc",
  );
  assert.throws(() => creditsTarget(undefined, "host unavailable"), /target triple/);
});

test("credits match exact Cargo identities and require every shipped executable", () => {
  const roots = ["transmog-desktop", "transmog", "transmog-script-host", "transmog-preview-worker"];
  const selected = [...roots, "runtime"].map((name) => ({ name, version: "1.0.0" }));
  const tree =
    selected.map((pkg) => `${pkg.name} v${pkg.version}`).join("\n") + "\nruntime v1.0.0 (*)\n";
  const metadata = {
    packages: [
      ...selected,
      { name: "build-only", version: "1.0.0" },
      { name: "runtime", version: "0.9.0" },
    ],
  };
  assert.deepEqual(shippedCargoPackages(metadata, tree), selected);
  assert.throws(
    () => shippedCargoPackages(metadata, "transmog-desktop v1.0.0"),
    /Missing shipped Cargo root/,
  );
  assert.throws(
    () => shippedCargoPackages({ packages: [...selected, selected[0]] }, tree),
    /ambiguous/,
  );
  assert.throws(
    () => shippedCargoPackages(metadata, tree + "missing v1.0.0\n"),
    /Missing or ambiguous/,
  );
});

test("Windows credits exclude build tools and foreign targets while retaining both helpers", () => {
  for (const target of ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"]) {
    const metadata = shippedCargoMetadata(repository, target);
    const names = new Set(metadata.packages.map((pkg) => pkg.name));
    for (const name of [
      "cc",
      "cmake",
      "bindgen",
      "core-foundation",
      "objc2",
      "gtk",
      "jni",
      "serde_derive",
      "microsoft-webui",
      "microsoft-webui-parser",
      "microsoft-webui-discovery",
    ])
      assert.ok(!names.has(name), `${target} credits include ${name}`);
    for (const name of ["transmog-desktop", "transmog", "v8", "image", "quiche"])
      assert.ok(names.has(name), `${target} credits omit ${name}`);
    const otherArchitecture = target.startsWith("aarch64")
      ? "windows_x86_64_msvc"
      : "windows_aarch64_msvc";
    assert.ok(!names.has(otherArchitecture), `${target} credits include ${otherArchitecture}`);
  }
});

test("browser credits follow surviving inputs across main and worker bundles", () => {
  const root = resolve("credits-fixture");
  const outputs = {
    "app.js": {
      inputs: {
        "src/index.ts": { bytesInOutput: 5 },
        "node_modules/@scope/runtime/index.js": { bytesInOutput: 10 },
        "node_modules/removed/index.js": { bytesInOutput: 0 },
      },
    },
    "worker.js": {
      inputs: {
        "node_modules/@scope/runtime/worker.js": { bytesInOutput: 10 },
        "node_modules/parent/node_modules/helper/index.js": { bytesInOutput: 10 },
      },
    },
    "document.css": {
      inputs: { "src/document.css": { bytesInOutput: 100 } },
    },
  };
  const inputs = {
    "src/document.css": {
      imports: [
        { kind: "url-token", path: "node_modules/font-only/notice.ttf" },
        {
          kind: "url-token",
          path: "https://example.invalid/node_modules/external/font.ttf",
          external: true,
        },
      ],
    },
    "node_modules/removed/index.js": {
      imports: [{ kind: "url-token", path: "node_modules/unused/font.ttf" }],
    },
  };
  assert.deepEqual(bundledNpmPackages(root, outputs, inputs), [
    "node_modules/@scope/runtime",
    "node_modules/font-only",
    "node_modules/parent/node_modules/helper",
  ]);
  assert.throws(
    () =>
      bundledNpmPackages(root, {
        "app.js": {
          inputs: {
            "../node_modules/unlocked/index.js": { bytesInOutput: 10 },
          },
        },
      }),
    /outside the UI lockfile/,
  );
  assert.throws(() => bundledNpmPackages(root), /bundle inputs/);
});

test("bundled copies of a development dependency share a credit and retain every notice", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "transmog-credits-"));
  try {
    const helper = join(temporary, "node_modules/helper");
    const nestedHelper = join(temporary, "node_modules/parent/node_modules/helper");
    await mkdir(helper, { recursive: true });
    await mkdir(nestedHelper, { recursive: true });
    const text = "Copyright bundled helper author.\nPermission notice retained in full.\n";
    const additional = "Copyright additional helper contributor.\n";
    await writeFile(join(helper, "LICENSE"), text);
    await writeFile(join(nestedHelper, "LICENSE"), text);
    await writeFile(join(nestedHelper, "NOTICE"), additional);
    await writeFile(
      join(temporary, "package-lock.json"),
      JSON.stringify({
        packages: {
          "node_modules/helper": { version: "1.0.0", dev: true, license: "MIT" },
          "node_modules/parent/node_modules/helper": {
            version: "1.0.0",
            dev: true,
            license: "MIT",
          },
          "node_modules/build-tool": { version: "1.0.0", dev: true },
          "node_modules/foreign-optional": { version: "1.0.0", os: ["darwin"] },
        },
      }),
    );
    const outputs = {
      "worker.js": {
        inputs: {
          "node_modules/helper/index.js": { bytesInOutput: 10 },
          "node_modules/parent/node_modules/helper/index.js": { bytesInOutput: 10 },
        },
      },
    };
    const entries = await npmCredits(temporary, outputs);
    assert.deepEqual(
      entries.map((entry) => entry.name),
      ["helper"],
    );
    assert.ok(entries[0].text.includes(text));
    assert.ok(entries[0].text.includes(additional));
    assert.equal(entries[0].text.split(text).length - 1, 1);
    const lockPath = join(temporary, "package-lock.json");
    const lock = JSON.parse(await readFile(lockPath, "utf8"));
    lock.packages["node_modules/parent/node_modules/helper"].license = "Apache-2.0";
    await writeFile(lockPath, JSON.stringify(lock));
    await assert.rejects(npmCredits(temporary, outputs), /Conflicting bundled license metadata/);
  } finally {
    assert.equal(dirname(resolve(temporary)), resolve(tmpdir()));
    await rm(temporary, { recursive: true, force: true });
  }
});

test("a runtime crate keeps a project notice supplied by an excluded build sibling", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "transmog-credits-"));
  try {
    await mkdir(join(temporary, "runtime"));
    await mkdir(join(temporary, "tool"));
    const text = "Copyright shared project authors.\nComplete Apache notice.\n";
    await writeFile(join(temporary, "tool/LICENSE"), text);
    const runtime = {
      id: "runtime",
      name: "runtime-notice-fixture",
      version: "1.0.0",
      source: "registry+https://github.com/rust-lang/crates.io-index",
      repository: "https://example.invalid/shared-project",
      license: "Apache-2.0",
      manifest_path: join(temporary, "runtime/Cargo.toml"),
    };
    const tool = {
      ...runtime,
      id: "tool",
      name: "build-only-fixture",
      manifest_path: join(temporary, "tool/Cargo.toml"),
    };
    const entries = await cargoCredits(repository, {
      packages: [runtime],
      noticeSources: [runtime, tool],
    });
    assert.deepEqual(
      entries.map((entry) => entry.name),
      [runtime.name],
    );
    assert.equal(entries[0].text, text);
    await assert.rejects(
      cargoCredits(repository, {
        packages: [{ ...runtime, license: "MIT" }],
        noticeSources: [tool],
      }),
      /Missing or stale bundled license/,
    );
  } finally {
    assert.equal(dirname(resolve(temporary)), resolve(tmpdir()));
    await rm(temporary, { recursive: true, force: true });
  }
});

test("credits retain the shipped MAPI enum data and its complete license", async () => {
  const entries = await extensionCredits(repository);
  assert.equal(entries.length, 1);
  assert.equal(entries[0].name, "Microsoft MAPI protocol enum data");
  assert.equal(entries[0].version, "7a76470574377d14cae612c507f45b34d70e05a0");
  assert.equal(
    entries[0].text,
    await readFile(resolve(repository, "third_party/mapi-inspector/LICENSE"), "utf8"),
  );
});

test("the SWC sourcemap fork retains its BSD notice instead of the monorepo Apache notice", async () => {
  const metadata = shippedCargoMetadata(repository, "x86_64-pc-windows-msvc");
  const pkg = metadata.packages.find((pkg) => pkg.name === "swc_sourcemap");
  assert.ok(pkg);
  const entries = await cargoCredits(repository, { ...metadata, packages: [pkg] });
  assert.equal(entries[0].license, "BSD-3-Clause");
  assert.match(entries[0].text, /Copyright \(c\) 2016 by Armin Ronacher/);
  assert.match(entries[0].text, /Redistribution and use in source and binary forms/);
  assert.doesNotMatch(entries[0].text, /Apache License/);
  await assert.rejects(
    cargoCredits(repository, { packages: [{ ...pkg, license: "MIT" }], noticeSources: [] }),
    /Missing or stale bundled license/,
  );
});

test("credits retain each vendored transport and its complete upstream license", async () => {
  const entries = await cargoCredits(repository, {
    packages: [...packages, { id: "app", name: "transmog-desktop", source: null }],
  });
  assert.equal(entries.length, 3);
  for (const pkg of packages) {
    const entry = entries.find((entry) => entry.name === pkg.name);
    assert.ok(entry, "Missing " + pkg.name);
    assert.equal(entry.version, pkg.version);
    const license = await readFile(
      resolve(
        repository,
        "third_party/rust-transport/vendor",
        pkg.name,
        pkg.name === "quiche" ? "COPYING" : "LICENSE",
      ),
      "utf8",
    );
    assert.ok(entry.text.includes(license), "Incomplete license for " + pkg.name);
  }
});

test("credits retain the patched SWC parser and reject substituted copies", async () => {
  const directory = resolve(repository, "third_party/swc-ecma-parser");
  const receipt = JSON.parse(await readFile(join(directory, "transmog-snapshot.json"), "utf8"));
  const parser = {
    id: "swc_ecma_parser",
    name: "swc_ecma_parser",
    version: receipt.source.version,
    source: null,
    license: "Apache-2.0",
    manifest_path: join(directory, "Cargo.toml"),
  };
  const entries = await cargoCredits(repository, { packages: [parser] });
  assert.equal(entries.length, 1);
  assert.ok(entries[0].text.includes(await readFile(join(directory, "LICENSE"), "utf8")));
  for (const change of [
    { source: "registry+https://github.com/rust-lang/crates.io-index" },
    { version: "0.0.0" },
    { manifest_path: resolve(repository, "Cargo.toml") },
  ]) {
    await assert.rejects(
      cargoCredits(repository, { packages: [{ ...parser, ...change }] }),
      /Unreviewed SWC parser dependency/,
    );
  }
});

test("credits reject transports with a different source, version, or manifest", async () => {
  for (const pkg of packages) {
    for (const change of [
      { source: "registry+https://github.com/rust-lang/crates.io-index" },
      { version: "0.0.0" },
      { manifest_path: resolve(repository, "Cargo.toml") },
    ]) {
      await assert.rejects(
        cargoCredits(repository, { packages: [{ ...pkg, ...change }] }),
        /Unreviewed transport dependency/,
      );
    }
  }
});

test("credits retain the native V8 fork and reject substituted engine sources", async () => {
  const directory = resolve(repository, "third_party/rusty-v8");
  const pin = JSON.parse(await readFile(join(directory, "source.json"), "utf8"));
  const engine = {
    id: "v8",
    name: "v8",
    version: pin.version,
    source: null,
    license: "MIT",
    manifest_path: join(directory, "Cargo.toml"),
  };
  const entries = await cargoCredits(repository, { packages: [engine] });
  assert.equal(entries.length, 1);
  for (const license of ["LICENSE", "LICENSE-V8"])
    assert.ok(entries[0].text.includes(await readFile(join(directory, license), "utf8")));
  for (const change of [
    { source: "registry+https://github.com/rust-lang/crates.io-index" },
    { version: "0.0.0" },
    { manifest_path: resolve(repository, "Cargo.toml") },
  ])
    await assert.rejects(
      cargoCredits(repository, { packages: [{ ...engine, ...change }] }),
      /Unreviewed V8 dependency/,
    );
});

test("credits accept a checkout reached through a directory alias", async () => {
  const temporary = await mkdtemp(join(tmpdir(), "transmog-credits-"));
  try {
    const alias = join(temporary, "checkout");
    await symlink(repository, alias, process.platform === "win32" ? "junction" : "dir");
    const entries = await cargoCredits(alias, { packages });
    assert.deepEqual(
      entries.map((entry) => entry.name),
      packages.map((pkg) => pkg.name),
    );
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});
