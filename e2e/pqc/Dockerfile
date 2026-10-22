ARG BASE_IMAGE=transmog-linux-tools:rust-1.99.1-nightly-2026-10-01
FROM node:24-bookworm-slim AS node-runtime
FROM ${BASE_IMAGE}

COPY --from=node-runtime /usr/local/bin/node /usr/local/bin/
COPY --from=node-runtime /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends libnss3-tools \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/pqc-browser
COPY package.json package-lock.json ./
RUN npm ci && node node_modules/playwright/cli.js install --with-deps chrome
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends dbus gnome-keyring \
    && rm -rf /var/lib/apt/lists/*
COPY pqc-browser.mjs pqc-live.mjs ./
WORKDIR /work
