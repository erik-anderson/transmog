FROM rustlang/rust:nightly-bookworm@sha256:91f0dc0a9033b793e10c04fe157b55c4189b57f11a266cdaec6e8d5ed7e68576

ENV PATH="/usr/local/cargo/bin:${PATH}"

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        clang \
        binutils \
        cmake \
        nasm \
        ninja-build \
        perl \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

RUN rustup toolchain install nightly-2026-10-01 --profile minimal \
    && rustup component add llvm-tools-preview --toolchain nightly-2026-10-01 \
    && cargo +nightly-2026-10-01 install cargo-fuzz --version 0.13.2 --locked

ENV CC=clang \
    CXX=clang++ \
    CFLAGS="-fsanitize=address -fno-omit-frame-pointer -fsanitize-coverage=inline-8bit-counters,pc-table,trace-cmp" \
    CMAKE_GENERATOR=Ninja

WORKDIR /work
