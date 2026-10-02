# Frontend panel

This panel is the starter React shell embedded by the generated Aster service. It is intentionally
small: product projects should keep route paths, generated API types, page components, and reusable
UI pieces in separate modules instead of wiring everything through `main.tsx`.

## Commands

```bash
bun install --frozen-lockfile
bun run dev
bun run check
bun run test
bun run build
```

Type checking uses the stable TypeScript 7 `tsc` CLI. Dependency resolution enforces a 24-hour
minimum release age through `bunfig.toml`; keep updates on established upstream packages and do not
bypass that supply-chain delay merely to take a same-day release.

## OpenAPI types

The backend OpenAPI test refreshes the tracked `generated/openapi.json`. Generate TypeScript types
from it with:

```bash
bun run generate-api
```

Generated OpenAPI types are written to `src/types/api.generated.ts`. Application code should import
from `src/types/api.ts`, which mirrors the wrapper style used by the reference Aster frontends. CI
regenerates both files and rejects drift.

The generator runs in a pinned TypeScript 6 environment through `bunx`: openapi-typescript still
uses the JavaScript compiler API that TypeScript 7 no longer exports. Frontend type checking and
builds continue to use TypeScript 7.
