# C++ application embedding example

This consumer uses only the installed public headers and library. It needs C and
C++20 compilers and CMake 3.24 or newer; Rust and Crubit are needed only to build
the SDK.

```sh
cmake -S examples/cpp-embedding -B .local/cpp-consumer -DCMAKE_PREFIX_PATH="<SDK_OUTPUT>" -DCMAKE_BUILD_TYPE=Debug
cmake --build .local/cpp-consumer --config Debug
ctest --test-dir .local/cpp-consumer -C Debug --output-on-failure
```

Run from the repository root and replace `<SDK_OUTPUT>` with the SDK's native
install directory. Choose a compiler compatible with that library's C++ ABI;
see the [qualified configurations](../../sdk/README.md#build-and-consume).
`--config` and `-C` also select Debug for multi-configuration CMake generators.

Run `transmog_cpp_embedding` to start a loopback HTTP proxy, send traffic through
its printed listener, then press Enter to list captured rows and shut down.
It reads the real ProductState, disables HTTPS decryption and system proxy
configuration, and starts from the shared Rust request defaults. Only explicitly
proxied traffic reaches it. `--self-test` exercises typed inputs/results, errors,
traffic views, ownership, and the C header against the installed library.

The example copies generated views to change fields. Their text and arrays
remain borrowed from the result owner until the call returns. Results can
outlive their application. See the [SDK contract](../../sdk/README.md) for
callbacks, body streaming, no-throw behavior, and the same-build ABI restriction.
