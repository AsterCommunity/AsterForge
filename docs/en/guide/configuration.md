# Configuration

AsterForge separates configuration into static config and runtime config. This split matters because not every setting can safely change online.

## Static Config

Static config is loaded from `data/config.toml` by default. If the file does not exist, the first startup creates it.

Example:

```toml
[server]
host = "127.0.0.1"
port = 3000
workers = 0
temp_dir = ".tmp"
start_mode = "primary"

[database]
url = "sqlite://asterforge.db?mode=rwc"
pool_size = 10
retry_count = 3

[auth]
jwt_secret = "replace-with-a-long-random-secret"
mfa_secret_key = "replace-with-another-long-random-secret"
bootstrap_insecure_cookies = false
```

The full example is in `config.example.toml` at the repository root.

Static config is suitable for:

- HTTP bind host and port.
- Database URL, pool size, and retry count.
- JWT and MFA secrets.
- Node startup mode.
- Cache backend and Redis URL.
- Logging format and file path.
- Initial rate limit and trusted proxy settings.

## Environment Overrides

Use `ASTER__...` environment variables to override static config. Nested keys use double underscores:

```bash
ASTER__SERVER__HOST=0.0.0.0
ASTER__SERVER__PORT=3000
ASTER__SERVER__START_MODE=primary
ASTER__DATABASE__URL='sqlite:///data/asterforge.db?mode=rwc'
ASTER__AUTH__JWT_SECRET='replace-with-a-long-random-secret'
```

In containers, keep secrets, database URLs, and bind settings in environment variables or a secret manager. Do not bake them into the image.

## Path Resolution

The default runtime directory is `data/`. When the config file lives at `data/config.toml`, relative paths resolve under `data/`.

For example:

```toml
[database]
url = "sqlite://asterforge.db?mode=rwc"

[server]
temp_dir = ".tmp"
```

resolves to paths like:

```text
data/asterforge.db
data/.tmp
```

This keeps local development and container volume mounts straightforward. In production, make sure `data/` or container `/data` is writable.

## Runtime Config

Runtime config is stored in the `system_config` table and managed through the Admin Config API or admin panel.

Runtime config is suitable for:

- Site name, public URL, and branding-related values.
- CORS, registration policy, and local email policy.
- Audit retention and maintenance parameters.
- Feature switches that do not require process restart.

It is not suitable for:

- Database URL.
- Bind address and port.
- JWT or MFA secrets.
- Node mode.

Those values usually require restart or must be known before startup.

## Admin Config API

Common endpoints:

```text
GET    /api/v1/admin/config
GET    /api/v1/admin/config/schema
GET    /api/v1/admin/config/{key}
PUT    /api/v1/admin/config/{key}
DELETE /api/v1/admin/config/{key}
```

`/schema` returns runtime config schema data. The frontend can use it to render forms, validate input, and show descriptions.

When adding a runtime config key, also add:

- Definition and default value.
- Schema metadata.
- Normalizer or validator.
- Admin UI coverage.
- Audit log for related operations.

## Auditing Config Changes

Admin config changes should create audit records. Details should keep the key, old value summary, new value summary, and change source. The presentation layer should provide stable frontend display data.

Do not make the frontend parse raw details strings. That gets brittle as the number of config keys grows.
