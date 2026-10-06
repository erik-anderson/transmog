# .NET 10 embedding sample

This educational sample demonstrates a .NET console application calling
Transmog in-process. It is deliberately small: the managed app accepts one URL,
an ordered set of allowed HTTP protocols, finite response/time limits, and an
output path. It performs a `GET`, writes the raw response body to that path, and
prints status, negotiated protocol, headers, trailers, and attempt evidence as
JSON.

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
  --protocols h2,h1 `
  --output ./example-response.bin
```

Protocol values are `h1`, `h2`, and `h3`. Their order is the attempt order. For
example, `--protocols h3,h2,h1` tries direct HTTP/3 first and falls back to
HTTP/2 and then HTTP/1.1 if no response has begun. HTTP/3 requires HTTPS. The
default is `h2,h1` so a server without QUIC does not incur an HTTP/3 timeout.

Additional options:

- `--max-response-bytes`: one to 268,435,456 bytes; default 16 MiB;
- `--timeout-seconds`: one to 300 seconds; default 30; and
- `--overwrite`: replace an existing output file instead of failing safely.

The sample makes one GET, does not follow redirects, and writes the response
body exactly as delivered by the origin. It does not decode content encodings.
TLS uses a snapshot of the operating system's trust roots and retains normal
certificate and endpoint verification.

## Why the bridge is narrow

The native layer exposes no Rust layout, allocator, async future, Hyper type, or
quiche type to managed code. A production managed package would likely add a
long-lived runtime handle, cancellation, streaming callbacks or managed streams,
structured configuration/result types, package-specific ABI compatibility, and
RID-specific native assets. This sample intentionally proves only the basic
embedding shape before those product decisions are made.
