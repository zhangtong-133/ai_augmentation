# 保留原来 MinIO / mc 的发布版本，从固定官方源码构建，避免依赖已拒绝匿名拉取的镜像仓库。
FROM golang:1.24.2-bookworm AS builder
ENV CGO_ENABLED=0 GOTOOLCHAIN=local
# 限制冷构建并发，避免 CI 同时编译两个程序时耗尽内存。
ENV GOFLAGS=-p=2

FROM builder AS minio-builder
RUN --mount=type=cache,target=/go/pkg/mod --mount=type=cache,target=/root/.cache/go-build \
    go install -trimpath -ldflags "-s -w -X github.com/minio/minio/cmd.Version=2025-04-22T22:12:26Z -X github.com/minio/minio/cmd.ReleaseTag=RELEASE.2025-04-22T22-12-26Z -X github.com/minio/minio/cmd.CommitID=0d7408fc9969caf07de6a8c3a84f9fbb10a6739e -X github.com/minio/minio/cmd.ShortCommitID=0d7408fc9969 -X github.com/minio/minio/cmd.CopyrightYear=2025" \
      github.com/minio/minio@v0.0.0-20250422221226-0d7408fc9969 \
    && cp /go/pkg/mod/github.com/minio/minio@v0.0.0-20250422221226-0d7408fc9969/LICENSE /LICENSE

FROM builder AS mc-builder
RUN --mount=type=cache,target=/go/pkg/mod --mount=type=cache,target=/root/.cache/go-build \
    go install -trimpath -ldflags "-s -w -X github.com/minio/mc/cmd.Version=2025-04-16T18:13:26Z -X github.com/minio/mc/cmd.ReleaseTag=RELEASE.2025-04-16T18-13-26Z -X github.com/minio/mc/cmd.CommitID=b00526b153a31b36767991a4f5ce2cced435ee8e -X github.com/minio/mc/cmd.ShortCommitID=b00526b153a3 -X github.com/minio/mc/cmd.CopyrightYear=2025" \
      github.com/minio/mc@v0.0.0-20250416181326-b00526b153a3 \
    && cp /go/pkg/mod/github.com/minio/mc@v0.0.0-20250416181326-b00526b153a3/LICENSE /LICENSE

FROM debian:bookworm-slim AS runtime
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

FROM runtime AS minio
LABEL org.opencontainers.image.source="https://github.com/minio/minio" \
      org.opencontainers.image.revision="0d7408fc9969caf07de6a8c3a84f9fbb10a6739e" \
      org.opencontainers.image.licenses="AGPL-3.0-only"
COPY --from=minio-builder /go/bin/minio /usr/local/bin/minio
COPY --from=minio-builder /LICENSE /usr/share/licenses/minio/LICENSE
ENTRYPOINT ["minio"]

FROM runtime AS mc
LABEL org.opencontainers.image.source="https://github.com/minio/mc" \
      org.opencontainers.image.revision="b00526b153a31b36767991a4f5ce2cced435ee8e" \
      org.opencontainers.image.licenses="AGPL-3.0-only"
COPY --from=mc-builder /go/bin/mc /usr/local/bin/mc
COPY --from=mc-builder /LICENSE /usr/share/licenses/mc/LICENSE
ENTRYPOINT ["mc"]
