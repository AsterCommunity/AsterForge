# Frontend panel

This panel is the starter React shell embedded by the generated Aster service. It is intentionally
small: product projects should keep route paths, generated API types, page components, and reusable
UI pieces in separate modules instead of wiring everything through `main.tsx`.

## Commands

```bash
bun install
bun run dev
bun run check
bun run test
bun run build
```

## OpenAPI types

The backend OpenAPI test writes `generated/openapi.json`. Generate TypeScript types from it with:

```bash
bun run generate-api
```

Generated OpenAPI types are written to `src/types/api.generated.ts`. Application code should import
from `src/types/api.ts`, which mirrors the wrapper style used by the reference Aster frontends.
