# syntax=docker/dockerfile:1
# Dependencies are built in their own layer (cargo-chef), so a code-only
# change rebuilds in minutes, not the whole DataFusion tree.
FROM lukemathwalker/cargo-chef:latest-rust-1-slim-bookworm AS chef
WORKDIR /src

FROM chef AS plan
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=plan /src/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p openlens -p openlens-cli \
 && mkdir -p /out/data
# Country lookups (#225): the free DB-IP Lite country file (CC BY 4.0),
# this month's or last month's. Rebuilt monthly (rebuild-stable.yml). The
# image still works without it (countries stay unknown).
RUN mkdir -p /out/share \
 && for m in "$(date -u +%Y-%m)" "$(date -u -d "$(date -u +%Y-%m-15) -1 month" +%Y-%m)"; do \
      wget -q -O /out/share/dbip-country-lite.mmdb.gz "https://download.db-ip.com/free/dbip-country-lite-$m.mmdb.gz" && break; \
      rm -f /out/share/dbip-country-lite.mmdb.gz; \
    done; ls -la /out/share

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/openlens /src/target/release/openlens-cli /usr/local/bin/
COPY --from=build /out/share /usr/share/openlens
# The image runs as nonroot (uid 65532): the data dir must be theirs, so a
# fresh named volume inherits the right owner.
COPY --from=build --chown=65532:65532 /out/data /data
ENV OPENLENS_ADDR=0.0.0.0:8686 \
    OPENLENS_DATA_DIR=/data
EXPOSE 8686
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/openlens"]
