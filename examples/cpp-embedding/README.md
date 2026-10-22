# C++ application embedding example

This consumer uses only the installed public headers and library. It needs a
C++20 compiler and CMake; Rust and Crubit are needed only to build the SDK.

```sh
cmake -S examples/cpp-embedding -B .local/cpp-consumer -DCMAKE_PREFIX_PATH=<SDK_OUTPUT>
cmake --build .local/cpp-consumer
ctest --test-dir .local/cpp-consumer --output-on-failure
```

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
