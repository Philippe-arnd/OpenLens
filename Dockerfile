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
# this month's or last month's. GEOIP_MONTH (set by CI) changes the layer
# every month, so the monthly rebuild refreshes it. Without it the image
# still works: countries stay unknown (CI checks that it is there).
FROM debian:bookworm-slim AS geo
ARG GEOIP_MONTH=
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends curl ca-certificates > /dev/null \
 && mkdir -p /out && m="${GEOIP_MONTH:-$(date -u +%Y-%m)}" \
 && for x in "$m" "$(date -u -d "$m-15 -1 month" +%Y-%m)"; do \
      curl -fsSL --retry 3 -o /out/dbip-country-lite.mmdb.gz "https://download.db-ip.com/free/dbip-country-lite-$x.mmdb.gz" && break; \
      rm -f /out/dbip-country-lite.mmdb.gz; \
    done; ls -la /out

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/openlens /src/target/release/openlens-cli /usr/local/bin/
COPY --from=geo /out /usr/share/openlens
# The image runs as nonroot (uid 65532): the data dir must be theirs, so a
# fresh named volume inherits the right owner.
COPY --from=build --chown=65532:65532 /out/data /data
ENV OPENLENS_ADDR=0.0.0.0:8686 \
    OPENLENS_DATA_DIR=/data
EXPOSE 8686
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/openlens"]
