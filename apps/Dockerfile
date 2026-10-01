FROM rust:1.96-bookworm AS builder

WORKDIR /workspace
# 使用指定版本镜像自带的工具链。复制开发环境的 "stable" 覆盖配置
# 会在构建时额外下载一套版本可能变化的工具链。
COPY Cargo.toml Cargo.lock ./
COPY apps ./apps
COPY crates ./crates

ARG APP
RUN cargo build --locked --release --package "$APP" \
    && mkdir /workspace/admin-bin \
    && if [ "$APP" = "api-server" ]; then cp /workspace/target/release/reply-operations /workspace/target/release/model-agent-operations /workspace/target/release/scheduler-operations /workspace/target/release/mcp-operations /workspace/target/release/feed-operations /workspace/target/release/learning-operations /workspace/admin-bin/; fi

FROM debian:bookworm-slim AS runtime
ARG APP
ENV APP_BINARY="$APP"
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN export http_proxy="${HTTP_PROXY:-}" https_proxy="${HTTPS_PROXY:-}" no_proxy="${NO_PROXY:-}" \
    && sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Retries=3 -o Acquire::https::Timeout=30 update \
    && apt-get -o Acquire::Retries=3 -o Acquire::https::Timeout=30 install -y --no-install-recommends poppler-utils util-linux \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --uid 10001 app
COPY --from=builder "/workspace/target/release/$APP" "/usr/local/bin/$APP"
COPY --from=builder /workspace/admin-bin/ /usr/local/bin/
USER app
ENTRYPOINT ["/bin/sh", "-c", "exec /usr/local/bin/$APP_BINARY"]
