# Changelog

All notable changes to AsterForge will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-06-07

First public release of AsterForge, a reusable Rust + React service foundation for Aster projects.

### Added

#### Foundation

- Rust workspace with main server binary and supporting crates.
- Authentication system: JWT, sessions, MFA, and external auth providers (OAuth2/OIDC).
- Audit logging with structured presentation and async batch writer.
- Background task system with lane-based dispatch, automatic retry, and heartbeat tracking.
- Runtime configuration with hot-reload support and admin UI management.
- Repository pattern with pagination, filtering, and multi-database support (SQLite / PostgreSQL / MySQL).
- Metrics collection with Prometheus recorder and system resource tracking.
- Multi-tier rate limiting (auth / API / write operations).
- CORS and CSRF protection with configurable policies.
- Health check system with readiness and liveness endpoints.
- Cache abstraction with memory, Redis, and noop backends.
- Comprehensive error handling with structured error codes.
- SeaORM entities for users, sessions, audit logs, tasks, config, and external auth.
- Foundation schema migration with cascading relationships and automatic migration runner.
- Connection pooling with retry logic and health checks.

#### Mail Delivery

- Durable `mail_outbox` table with retry semantics.
- SMTP mail sender with configurable security (TLS / STARTTLS / none) via `lettre`.
- 7 built-in HTML email templates: registration activation, password reset, password reset notice, login email code, contact change confirmation, contact change notice, external auth email verification.
- Handlebars-style template variable system with link construction.
- `mail-outbox-dispatch` background task with automatic retry on failure.
- Config service extensions for SMTP host, port, credentials, and from address.
- Admin "test mail" config action for verifying delivery from the control panel.
- Mail audit logging for sent and failed deliveries, with frontend presentation labels.
- `MailNotConfigured` and `MailDeliveryFailed` error types with service-unavailable HTTP status.
- `mail_sender` runtime state trait, initialized in both primary and follower startup.
- Frontend OpenAPI bindings for mail configuration and action endpoints.
- Pre-mail audit configs automatically fall back to the all-actions audit scope.

#### Frontend

- React SPA with TypeScript, Vite, and shadcn/ui components.
- Authentication UI: login, register, MFA, external auth.
- Admin panels for system config, audit logs, tasks, and external auth providers.
- Real-time service diagnostics panel with live endpoint testing.
- Cross-tab session synchronization via BroadcastChannel.
- API catalog driven by generated OpenAPI specs.
- Form validation with comprehensive input helpers.
- Responsive layout with sidebar navigation and module rail.
- CSRF token handling and bearer auth for protected routes.

#### Infrastructure

- Docker multi-stage build with minimal runtime image.
- `docker-compose.yml` for single-command deployment.
- GitHub Actions CI/CD: Rust tests, frontend tests, E2E, Docker publish, performance, audit, release.
- `cargo-audit` integration for dependency vulnerability scanning.
- Automated OpenAPI spec generation from backend annotations.
- k6 performance test suite for runtime smoke testing.
- VitePress documentation site with GitHub Pages deployment.
- `cargo-generate` template support.

#### Documentation

- Deployment guides (Docker, manual).
- Developer documentation for architecture and extension.
- Configuration reference covering all available options.
- Authentication guide for local and external auth.
- Runtime operation guide for audit logs and background tasks.
- Mail delivery guide (English and Chinese) covering SMTP settings, templates, outbox, audit, troubleshooting.
- Developer mail runtime extension contracts (English and Chinese) covering template payloads, outbox semantics, config actions, audit details, and verification requirements.
- Security policy, code of conduct, and contribution guidelines.

#### Testing

- Integration test suite with isolated database per test.

### Changed

- VitePress deployment now triggers on `release: published` instead of push to `master`. Manual dispatch retained.

### Notes

- This is the first tagged release. The 0.x series is considered foundation-stage: minor versions may include breaking changes as the runtime surface stabilizes.
- Mail delivery is the only product feature introduced at 0.1.0; subsequent minor versions will layer additional runtime capabilities on the same foundation.

[0.1.0]: https://github.com/AsterCommunity/AsterForge/releases/tag/v0.1.0
