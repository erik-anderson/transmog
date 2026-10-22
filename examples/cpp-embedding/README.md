# C++ application embedding example

Build the opt-in SDK with `python scripts/build-sdk.py --test`. This also builds
this independent consumer using only the installed `Transmog::SDK` CMake target.
To build independently, set `CMAKE_PREFIX_PATH=<SDK_OUTPUT>` and use a matching
Clang/LLVM toolchain. Rust and Crubit are not consumer dependencies.

`transmog_cpp_embedding --self-test` exercises application creation, status,
product-state and session queries, traffic-view validation, callback subscription
creation, malformed input, move ownership, and final
shutdown through installed C++/C entry points without network access.

For an interactive run, pass two files:

```text
transmog_cpp_embedding <product-state.json> <proxy-start.json>
```

The first is the existing ProductState JSON returned by `Application::product_state`
(print it with `transmog_cpp_embedding --product-state`),
with `preferences.decryptHttps=false` and `configureSystemProxy=false`.
The second uses the existing ProxyStartRequest schema, for example:

```json
{"caCertificatePath":"","caPrivateKeyPath":"","listen":"127.0.0.1:0","route":"http1","allowRemoteClients":false,"recordingOnly":false}
```

The example prints the bound endpoint. Send HTTP traffic through it with a
separate client, then press Enter to query captured exchanges and stop.
The SDK uses the same application models and drain/shutdown semantics as the
desktop, and performs no host proxy or trust installation.
See [the SDK guide](../../sdk/README.md) and the [.NET example](../dotnet-embedding/README.md)
for the complete host-traffic demonstration and current binding scope.
