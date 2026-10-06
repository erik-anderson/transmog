# .NET 10 embedding sample

This educational sample demonstrates a .NET console application calling
Transmog in-process. It is deliberately small: the managed app accepts one URL,
a set of allowed HTTP protocols, finite response/time limits, and an output
path. It performs a `GET`, writes the raw response body to that path, and prints
status, negotiated protocol, headers, trailers, and attempt evidence as JSON.

The sample is a feasibility demonstration, not a stable managed SDK. Its native
C ABI is revisioned but remains a same-build interface that may change with the
repository. It does not launch the Transmog CLI or communicate with a separate
proxy process.

## How it is layered

```text
.NET 10 console application
        | P/Invoke: bounded JSON request + opaque result handle
        v
transmog-dotnet-sample-native (Rust cdylib)
        | canonical Target, RequestHead, BodyStream, UpstreamPlan
        v
transmog-core UpstreamExecutor
        |
        +-- transmog-http (HTTP/1.1 or HTTP/2)
        +-- transmog-h3   (HTTP/3)
        |
        v
transmog-tls system-trust snapshot and BoringSSL policy
```

Rust owns the async runtime, protocol pools, TLS state, response bounds, and all
native allocations. C# copies body and metadata bytes while the opaque result
handle is live and then releases the handle exactly once.

## Build

Install the normal Transmog platform prerequisites plus the .NET 10 SDK. From
the repository root, build the Rust dynamic library and managed application and
copy the native library beside the managed executable:

```powershell
pwsh ./scripts/build-dotnet-sample.ps1
```

Use `-Configuration Release` for optimized binaries. On Windows the script
enters the repository's LLVM/Ninja environment automatically. Linux uses the
active Clang/LLVM toolchain. macOS remains unqualified even though the build
script recognizes its dynamic-library naming convention.

## Run

```powershell
dotnet run --no-build `
  --project ./examples/dotnet-embedding/Transmog.Embedding.Sample `
  -- `
  --url https://example.com/ `
  --protocols h1,h2 `
  --output ./example-response.bin
```

Protocol values are `h1`, `h2`, and `h3`; their order has no meaning. When both
`h1` and `h2` are allowed, Transmog offers `h2` and `http/1.1` together in the
TLS ALPN handshake and uses the protocol selected by the origin. A singleton
`h1` or `h2` set advertises only that protocol.

HTTP/3 uses QUIC and therefore cannot participate in the same TCP/TLS ALPN
handshake. In this one-shot sample, allowing `h3` causes one direct HTTP/3
attempt before the HTTP/2-and-HTTP/1.1 ALPN connection, regardless of the list
order. HTTP/3 requires HTTPS. The default is `h1,h2`, so an origin without QUIC
does not incur an HTTP/3 timeout.

Additional options:

- `--max-response-bytes`: one to 268,435,456 bytes; default 16 MiB;
- `--timeout-seconds`: one to 300 seconds; default 30; and
- `--overwrite`: replace an existing output file instead of failing safely.

The sample makes one GET, does not follow redirects, and writes the response
body exactly as delivered by the origin. It does not decode content encodings.
TLS uses a snapshot of the operating system's trust roots and retains normal
certificate and endpoint verification. Origin connections use the same bounded
Happy Eyeballs implementation and default policy as the desktop proxy; the
sample does not carry a second managed implementation.

## Why the bridge is narrow

The native layer exposes no Rust layout, allocator, async future, Hyper type, or
quiche type to managed code. A production managed package would likely add a
long-lived runtime handle, cancellation, streaming callbacks or managed streams,
structured configuration/result types, package-specific ABI compatibility, and
RID-specific native assets. This sample intentionally proves only the basic
embedding shape before those product decisions are made.
