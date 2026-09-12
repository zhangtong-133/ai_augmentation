FROM rust:1.96-bookworm AS builder

WORKDIR /workspace
# Use the toolchain bundled with the versioned image. Copying the developer's
# "stable" override would download a second, drifting toolchain during builds.
COPY Cargo.toml Cargo.lock ./
COPY apps ./apps
COPY crates ./crates

ARG APP
RUN cargo build --locked --release --package "$APP"

FROM debian:bookworm-slim AS runtime
ARG APP
ENV APP_BINARY="$APP"
RUN useradd --create-home --uid 10001 app
COPY --from=builder "/workspace/target/release/$APP" "/usr/local/bin/$APP"
USER app
ENTRYPOINT ["/bin/sh", "-c", "exec /usr/local/bin/$APP_BINARY"]
