# Development

## Requirements
- Rust stable (pinned by `rust-toolchain.toml`)
- Optional: `cargo-deny` (CI always runs it; `cargo xtask ci` says when it skipped it)
- Later (M1+): Node 24 for `sdk/js`, `trunk` for the Leptos UI

## Everyday commands
```sh
cargo xtask ci          # the full gate
cargo xtask layers      # layering only
cargo run -p openlens   # OPENLENS_ADDR=0.0.0.0:8686 by default
```

## Docker
```sh
docker compose up --build    # http://localhost:8686/healthz
```
The healthcheck lives in `docker-compose.yml`, not in the Dockerfile, so Coolify picks it up.
