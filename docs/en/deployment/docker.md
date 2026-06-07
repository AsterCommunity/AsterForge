# Docker Deployment

AsterForge includes a Dockerfile, Compose configuration, and GitHub Actions image workflow. Runtime state lives under `/data`; container images should not store mutable runtime data.

## Docker Compose

Minimal local start:

```bash
mkdir -p ./data
docker compose up -d
```

Logs:

```bash
docker compose logs -f
```

Stop:

```bash
docker compose down
```

`./data` is mounted to container `/data`. Database files, config files, and runtime state should live there.

## Local Build

```bash
docker build -t asterforge:local .
docker run --rm -p 3000:3000 -v "$(pwd)/data:/data" asterforge:local
```

With config overrides:

```bash
docker run --rm \
  -p 3000:3000 \
  -v "$(pwd)/data:/data" \
  -e ASTER__SERVER__HOST=0.0.0.0 \
  -e ASTER__SERVER__PORT=3000 \
  -e ASTER__DATABASE__URL='sqlite:///data/asterforge.db?mode=rwc' \
  -e ASTER__AUTH__JWT_SECRET='replace-with-a-long-random-secret' \
  asterforge:local
```

Do not use example secrets in production. JWT and MFA secrets should come from a secret manager or protected deployment variables.

## Reverse Proxy

Production deployments should run behind an HTTPS reverse proxy. The proxy should handle:

- TLS termination.
- Forwarded headers such as `X-Forwarded-For` and `X-Forwarded-Proto`.
- Request body size limits.
- Timeouts.
- Access logs.

Configure trusted proxies on the AsterForge side so the service does not trust forged forwarded headers from clients.

## Health Checks

Container orchestrators can use:

```text
GET /health
GET /health/ready
```

`/health` is process liveness. `/health/ready` means the service is ready to accept requests. Readiness should fail when the database is unavailable, migrations fail, or runtime is not ready.

## Image Publishing

`.github/workflows/docker-image.yml` publishes to GHCR by default:

```text
ghcr.io/astercommunity/asterforge
```

Tag pushes build amd64 and arm64 images and create a multi-arch manifest. Docker Hub is not published by default.

To publish Docker Hub as well, run the workflow manually and enable `publish_dockerhub`. The repository must define:

```text
DOCKERHUB_USERNAME
DOCKERHUB_TOKEN
```

This opt-in keeps template projects from accidentally pushing images to the wrong Docker Hub namespace.

## Primary And Follower

Single-instance deployments use the default:

```bash
ASTER__SERVER__START_MODE=primary
```

In multi-node deployments, only nodes responsible for global maintenance should run as primary. Follower nodes can serve HTTP traffic but skip dispatcher and cleanup loops.

## Backups

For the default SQLite deployment, back up at least:

```text
data/config.toml
data/asterforge.db
data/asterforge.db-shm
data/asterforge.db-wal
```

If using PostgreSQL or MySQL, follow that database's backup strategy and still keep `data/config.toml` plus other runtime files.
