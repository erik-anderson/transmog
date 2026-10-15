FROM transmog-pqc-browser
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends python3-hpack \
    && rm -rf /var/lib/apt/lists/*
COPY browser-fingerprint.mjs /opt/pqc-browser/browser-fingerprint.mjs
WORKDIR /work
