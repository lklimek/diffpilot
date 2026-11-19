# ADR-012: TypeScript toolchain (TypeScript 7 for `tsc`, TypeScript 6 for tools)

- Status: accepted (coordinator, 2026-11-16, WP2-WEB)
- Source: `web/package.json` (commit `6cf92b6`).

## Context
TypeScript 7 (`typescript@7.0.2`, the native compiler) has no classic compiler API. typescript-eslint 8.73.0 (the
latest release) needs that API and declares the peer range `typescript >=4.8.4 <6.1.0`; so does the wording check
(`scripts/check-wording.mjs`, plan §12.4).

## Decision
`web/package.json` installs two compilers:
- `"@typescript/native": "npm:typescript@7.0.2"`: TypeScript 7; its `tsc` is `node_modules/.bin/tsc`, so
  `npm run build` (`tsc --noEmit` before `vite build`) type-checks with TypeScript 7.
- `"typescript": "npm:@typescript/typescript6@6.0.2"`: every package that imports `typescript` (typescript-eslint,
  msw) gets TypeScript 6. `"@typescript/typescript6": "6.0.2"` is also a direct dependency; the wording check
  loads it from `web/node_modules` after `npm ci` (override: `CHECK_WORDING_TYPESCRIPT`).

## Consequences
- Lint and the wording check parse with TypeScript 6, the build type-checks with TypeScript 7; syntax that only
  TypeScript 7 accepts fails lint first.
- **Remove the alias** (make `typescript` TypeScript 7 and drop `@typescript/native`) once typescript-eslint's
  peer range includes TypeScript 7; port the wording check to a parser that works with TypeScript 7 in the same
  change, or keep `@typescript/typescript6` for it alone.
