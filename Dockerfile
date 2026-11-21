# syntax=docker/dockerfile:1
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p openlens

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/openlens /usr/local/bin/openlens
ENV OPENLENS_ADDR=0.0.0.0:8686
EXPOSE 8686
VOLUME ["/data"]
ENTRYPOINT ["/usr/local/bin/openlens"]
