# Runtime

AsterForge's runtime is designed to make startup, background tasks, audit writes, health checks, and shutdown reusable across services. Downstream projects can add domain modules without redesigning the common lifecycle.

## Startup Mode

`server.start_mode` controls node mode:

```toml
[server]
start_mode = "primary"
```

Available values:

- `primary`: the default mode. Starts the HTTP service, background task dispatcher, and periodic maintenance tasks.
- `follower`: initializes common runtime and HTTP service, but skips primary-only tasks.

Follower mode is useful for future read-only nodes, managed ingress nodes, or topologies where one primary node owns global maintenance.

## Primary-only Tasks

Primary nodes start these runtime loops:

- background task dispatcher
- system health check
- auth session cleanup
- external auth login flow cleanup
- audit log cleanup
- task artifact cleanup

These tasks own global maintenance. Running them on every node can cause duplicate claims, duplicate cleanup, or noisy audit records.

## Graceful Shutdown

When a shutdown signal is received, runtime coordinates shutdown:

```mermaid
sequenceDiagram
    participant OS as Signal
    participant Runtime as Runtime shutdown
    participant HTTP as HTTP server
    participant Tasks as Background tasks
    participant Audit as Audit manager
    participant DB as Database

    OS->>Runtime: shutdown requested
    Runtime->>HTTP: stop accepting new work
    Runtime->>Tasks: cancel loops and release leases
    Runtime->>Audit: flush buffered audit events
    Runtime->>DB: close database handles
```

The point is not to stop instantly. The point is to let important state reach storage. If task leases are not released or audit buffers are not flushed, the next startup may see stuck tasks or missing audit records.

## Background Dispatch

The background task system persists task state. The dispatcher claims runnable tasks, runtime writes heartbeat and lease state, and failures are classified for retry behavior.

When adding a task, decide:

- Whether the payload can be deserialized long term.
- Whether result and failure details are useful for administrators.
- Whether presentation codes exist for stable frontend display.
- Whether retry is safe and idempotent.

AsterForge does not assume regular users can see task records. The template provides admin-facing and runtime-facing task APIs only.

## Audit Writes

The audit manager supports buffered async writes. Request handlers can submit audit events without synchronously blocking on every database insert. During shutdown, runtime flushes the buffer to reduce loss risk.

When adding admin operations, authentication events, config changes, or maintenance actions, add audit records. Prefer structured details plus presentation mapping, not frontend parsing of raw JSON.

## Health Checks

Common endpoints:

```text
GET /health
GET /health/ready
GET /health/metrics
```

`/health` is a liveness check. `/health/ready` is a readiness check. `/health/metrics` requires the metrics feature and is intended for Prometheus-style scraping.

## Extension Rules

When adding runtime behavior, classify it first:

- Runs on every node: common startup.
- Runs only on one primary node: primary startup.
- Runs only on follower nodes: follower startup.
- Belongs to a product module: downstream runtime module.

Do not put product-specific periodic jobs directly into the shared startup path. Keep ownership clear so node modes stay understandable.
