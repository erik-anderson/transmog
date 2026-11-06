# Transmog V8 profile

This private binding fork uses rusty_v8 152.2.0's Rust interface and a native
source build from the checksum-pinned registry archive in `source.json`.
Only the binding sources are vendored here; `scripts/v8_native.py` verifies and
stages the full native source under `.tools/rusty-v8/sources`. Each archive,
native patch and ICU data combination has an immutable source directory.
Preparation validates a complete private staging tree before publishing it;
concurrent builds never patch an active source tree in place.

`profile.gn` selects Sparkplug, omitting TurboFan, Maglev, WebAssembly and
Temporal. Inspector, Intl, Unicode normalization, Unicode property expressions,
locale-sensitive casing/collation, and segmentation remain available.

`source.json` pins the ICU data omitted from the registry source archive.
`scripts/icu_data.py` removes only legacy charset-converter tables (`*.cnv`),
which V8 does not expose, and `nfkc_scf.nrm`, the unused NFKC Simple Casefold
mode [also removed upstream](https://chromium.googlesource.com/chromium/deps/icu/+/8cc91d9b6ab9991802fd208ee03a69714fd0251c).
Every other resource retains its original bytes, including all upstream locale,
collation, timezone, dictionary, property and normalization resources. The bundle
shrinks from 10,876,560 to 9,889,328 bytes; no additional locale restrictions apply.
Rust URL/IDNA handling is independent. See the
[runtime contract](../../docs/scripting.md#javascript-runtime).

`native.patch` keeps ICU/Wasm binding entry points conditional on native features,
omits Chromium's SDK debugger-DLL deployment step for this static archive, and
adapts the [Windows ExtendedMap layout fix used by workerd](https://github.com/cloudflare/workerd/blob/main/patches/v8/0035-Fix-ExtendedMap-layout-on-Windows.patch)
to the pinned V8 source. C++/Torque layout assertions remain enabled. The fix
places the interceptor flag/padding bytes in the base class's tagged slot.
The allocation-retry callback also uses the generic callback signature from newer
V8 sources, allowing Chromium Clang to build against the installed MSVC library.
The Windows CRT configuration follows Rust's `crt-static` target feature, keeping
V8 and the host on the same C/C++ runtime.

The build always compiles the native archive and generates matching Rust layout
bindings using the native adapter's defines, headers, standard library and ABI
settings. Upstream stock-archive overrides do not select another engine profile.
Native release artifacts are shared between Rust build profiles under
`target/native-v8` (or `CARGO_TARGET_DIR/native-v8`) with a process lock and a
source/profile/target/ABI key. GN, Ninja and Chromium Clang are pinned by the
reviewed upstream source and cached under `.tools/rusty-v8/tools`.
Their installation is locked across builds that share the host tools.
Generated Rust bindings stay in each Cargo build's own output directory.
Native preparation and compilation hold a cache lease shared with
`scripts/clean-disk.py --include-release`, which can reclaim older native
variants and prepared sources while preserving the current source and tools.

Use the portable environment wrapper for Cargo. `python scripts/cache-v8.py`
can prepare the native engine before other builds. The first build needs network
access to the pinned source/build tools; later builds reuse the verified sources
and native outputs. Full PDB retention and sandbox restrictions remain unchanged.

`bindings.patch` records the local binding/build/profile delta against the registry
archive; `transmog-snapshot.json` inventories this fork for build preflight and
supply-chain notices. Preserve the MIT binding license and BSD V8 license.
`LICENSE-THIRD-PARTY` retains the native component notices omitted from the
registry archive; their exact upstream URLs and hashes are in `native-licenses.json`.
