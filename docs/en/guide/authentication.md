# Authentication

AsterForge provides local authentication, session management, first-admin setup, and external auth provider scaffolding. Downstream projects can extend this with roles, permissions, organizations, or tenants.

## First Admin

After a new instance starts, create the first administrator:

```bash
curl -X POST http://127.0.0.1:3000/api/v1/auth/setup \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","email":"admin@example.com","password":"change-me-please"}'
```

This endpoint should only succeed while no administrator exists. After setup, admin pages and Admin APIs require an admin session.

## Local Login Flow

Common endpoints:

```text
POST /api/v1/auth/register
POST /api/v1/auth/login
POST /api/v1/auth/refresh
POST /api/v1/auth/logout
GET  /api/v1/auth/me
GET  /api/v1/auth/sessions
```

Login creates an auth session. Refresh, logout, and session listing all use stored session state. Expired or revoked sessions are cleaned up by the primary runtime loop.

## Cookie Security

Production deployments should run behind HTTPS and keep secure cookies enabled. For local HTTP-only testing:

```bash
ASTER__AUTH__BOOTSTRAP_INSECURE_COOKIES=true cargo run
```

Do not enable this in production. Use HTTPS, reverse proxy security headers, and correct trusted proxy configuration.

## External Auth

External auth providers support OIDC/OAuth2-style identity sources. Public auth endpoints:

```text
GET  /api/v1/external-auth/providers
POST /api/v1/external-auth/{provider}/start
GET  /api/v1/external-auth/{provider}/callback
```

Grouped compatibility paths:

```text
GET  /api/v1/auth/external-auth/providers
GET  /api/v1/auth/external-auth/{kind}/providers
POST /api/v1/auth/external-auth/{kind}/{provider}/start
GET  /api/v1/auth/external-auth/{kind}/{provider}/callback
```

Admin endpoints for provider management:

```text
GET    /api/v1/admin/external-auth/provider-kinds
GET    /api/v1/admin/external-auth/providers
POST   /api/v1/admin/external-auth/providers
GET    /api/v1/admin/external-auth/providers/{id}
PATCH  /api/v1/admin/external-auth/providers/{id}
DELETE /api/v1/admin/external-auth/providers/{id}
POST   /api/v1/admin/external-auth/providers/test
POST   /api/v1/admin/external-auth/providers/{id}/test
```

External auth login flows persist temporary state. Consumed or expired flows are cleaned up by the primary runtime.

## Audit Requirements

Authentication-related actions should be audited, especially:

- User login.
- Admin create, update, or delete external auth provider.
- Provider tests.
- Config changes that affect authentication policy.

Audit details must not leak secrets. When storing provider secrets, audit only summaries or status, not sensitive values.

## Downstream Extensions

If a project needs a richer permission model, keep it in the downstream domain:

- AsterForge keeps base identity and admin middleware.
- Downstream projects add roles, resource permissions, organization membership, or tenant boundaries.
- Every new Admin API uses admin middleware explicitly.
- User-visible data and admin-visible data are modeled separately.

Do not expose background task records to regular users by default. Task records may contain maintenance state, failure details, and internal payloads. If a product needs user-facing tasks, design explicit visibility rules.
