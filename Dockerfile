# Self-hosted LRCGET web server. Build with:
#   docker build -t lrcget-web .
# See README.md ("Self-hosting the web version") for running it.

# --- Frontend ---
FROM node:24-bookworm-slim AS frontend
# git is needed for the lrc-kit dependency, which is installed from GitHub
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY index.html vite.config.js tailwind.config.cjs postcss.config.cjs ./
COPY public ./public
COPY src ./src
RUN npm run build

# --- Server ---
FROM rust:1-bookworm AS server
WORKDIR /app/src-tauri
COPY src-tauri ./
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/src-tauri/target \
    cargo build --release --locked --bin lrcget-server --no-default-features --features server \
    && cp target/release/lrcget-server /usr/local/bin/lrcget-server

# --- Runtime ---
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /data /music \
    && chown 1000:1000 /data
COPY --from=server /usr/local/bin/lrcget-server /usr/local/bin/lrcget-server
COPY --from=frontend /app/dist /app/dist

ENV LRCGET_BIND=0.0.0.0:3000 \
    LRCGET_DATA_DIR=/data \
    LRCGET_WEB_DIR=/app/dist \
    LRCGET_ROOT=/music

# Override with `user:` in compose to match the owner of your music files,
# so exported .lrc/.txt files and embedded lyrics can be written.
USER 1000:1000
VOLUME /data
EXPOSE 3000
ENTRYPOINT ["lrcget-server"]
