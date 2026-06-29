# Stage 1: Build frontend
FROM node:24-alpine AS frontend

RUN npm install -g bun@latest

WORKDIR /build/frontend-panel
COPY frontend-panel/package.json frontend-panel/bun.lock* ./
RUN bun install --frozen-lockfile

COPY frontend-panel/ ./
RUN bun run build

# Stage 2: Build Rust binary
FROM rust:1.95-alpine AS builder

RUN apk add --no-cache build-base pkgconfig sqlite-dev curl

WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
COPY migration/ migration/
COPY build.rs ./

# Pre-build dependencies as a cache layer. The command may fail before all template sources are
# copied; dependency artifacts are still reused by the final build.
RUN mkdir src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

COPY src/ src/
COPY tests/ tests/
COPY --from=frontend /build/frontend-panel/dist/ frontend-panel/dist/

ARG CARGO_FEATURES="metrics"
ENV RUSTFLAGS="-C link-arg=-s"

RUN if [ -n "${CARGO_FEATURES}" ]; then \
      cargo build --release --features "${CARGO_FEATURES}"; \
    else \
      cargo build --release; \
    fi

# Stage 3: Alpine runtime
FROM alpine:3.23

RUN apk add --no-cache ca-certificates sqlite-libs wget && \
    addgroup -S -g 10001 aster && \
    adduser -S -D -H -u 10001 -G aster -s /sbin/nologin aster && \
    mkdir -p /data && \
    chown -R aster:aster /data

LABEL maintainer="AptS:1547 <apts-1547@esaps.net>"
LABEL org.opencontainers.image.title="{{project-name}}"
LABEL org.opencontainers.image.description="{{package_description}}"
LABEL org.opencontainers.image.source="https://github.com/AsterCommunity/AsterForge"
LABEL org.opencontainers.image.license="MIT"

COPY --from=builder /build/target/release/{{project-name}} /usr/local/bin/{{project-name}}
COPY config.example.toml /config.example.toml

VOLUME ["/data"]
EXPOSE {{server_port}}

WORKDIR /
ENV ASTER__SERVER__HOST=0.0.0.0

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD ["wget", "-q", "-O", "/dev/null", "http://127.0.0.1:{{server_port}}/health/ready"]

USER aster:aster

ENTRYPOINT ["/usr/local/bin/{{project-name}}"]
