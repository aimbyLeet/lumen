# lumen-server image. Build context: the repository root
# (deploy/build-server.ps1 builds it and packs the deployment archive).

# The installer devices update to: build-server.ps1
# passes its folder, `--build-context update=<folder>`; without it, none.
FROM scratch AS update

FROM node:22.12.0-bookworm-slim AS web
WORKDIR /web/admin
COPY web/admin/package.json web/admin/package-lock.json ./
RUN npm ci --no-fund --no-audit
COPY web/admin/ ./
# The look is Lumen Client's: its styles, fonts and small parts.
COPY web/client/src/ /web/client/src/
RUN npm run build

FROM rust:1.99-slim-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p lumen-server \
    && cp target/release/lumen-server /lumen-server

FROM debian:trixie-slim
COPY --from=build /lumen-server /usr/local/bin/lumen-server
COPY --from=web /web/admin/dist /usr/share/lumen/admin
COPY deploy/server/entrypoint.sh /usr/local/bin/entrypoint.sh
COPY --from=update / /usr/share/lumen/update/
# Admin commands (`docker compose exec server lumen-server access ...`) use the same database.
ENV LUMEN_DB=/data/lumen.db
ENV LUMEN_WEB=/usr/share/lumen/admin
ENV LUMEN_UPDATE=/usr/share/lumen/update
VOLUME /data
ENTRYPOINT ["/bin/sh", "/usr/local/bin/entrypoint.sh"]
CMD ["serve"]
