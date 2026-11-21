# syntax=docker/dockerfile:1
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p openlens \
 && mkdir -p /out/data

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/openlens /usr/local/bin/openlens
# The image runs as nonroot (uid 65532): the data dir must be theirs, so a
# fresh named volume inherits the right owner.
COPY --from=build --chown=65532:65532 /out/data /data
ENV OPENLENS_ADDR=0.0.0.0:8686 \
    OPENLENS_DATA_DIR=/data
EXPOSE 8686
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/openlens"]
