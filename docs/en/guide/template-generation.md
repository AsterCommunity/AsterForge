# Template Generation

AsterForge can be used as a `cargo-generate` starter repository, or copied directly and initialized with `./init.sh`.

## Using cargo-generate

Install:

```bash
cargo install cargo-generate
```

Generate a new project:

```bash
cargo generate --git https://github.com/AsterCommunity/AsterForge --name my-service
cd my-service
./init.sh
```

`cargo-generate.toml` filters common local artifacts, including:

- `target/`
- `data/`
- `tmp/`
- `coverage/`
- `frontend-panel/node_modules/`
- `frontend-panel/dist/`
- `frontend-panel/test-results/`
- `docs/node_modules/`
- `docs/.vitepress/dist/`
- `docs/.vitepress/cache/`

Project-level workflow configuration is kept in the template, including `.gcop`. These files are part of the engineering workflow that new projects should inherit.

## Using init.sh

`./init.sh` replaces project display name, crate name, slug, environment token, and repository URL.

Show options:

```bash
./init.sh --help
```

Non-interactive example:

```bash
./init.sh \
  --name MyService \
  --crate my_service \
  --slug myservice \
  --upper MYSERVICE \
  --repo https://github.com/AsterCommunity/MyService \
  --yes
```

Dry run:

```bash
./init.sh --name MyService --dry-run
```

The script skips build artifacts, runtime data, frontend dependencies, test output, and docs build cache. It only edits text files and only when old markers are present.

## Naming Rules

Keep these names aligned:

```text
Display name: MyService
Rust crate:   my_service
Flat slug:    myservice
Kebab slug:   my-service
Upper token:  MYSERVICE
```

`init.sh` can derive defaults from the display name, but real projects should pass explicit values so a temporary directory name does not affect the result.

## After Initialization

Run:

```bash
cargo fmt
cargo generate-lockfile
cargo check --bins
cd frontend-panel
bun install
bun run check
```

If OpenAPI changed:

```bash
cargo test --features openapi generate_openapi
cd frontend-panel
bun run generate-api
```

If docs changed:

```bash
cd docs
bun install
bun run docs:build
```

## Manual Review

Template initialization handles generic naming, not product decisions. Check at least:

- README and docs product text.
- `config.example.toml` defaults.
- Docker image names and GitHub Actions publish targets.
- Admin UI branding.
- OpenAPI annotations for new APIs.
- Audit logs for new admin operations.
- Presentation and cleanup policy for new background tasks.
