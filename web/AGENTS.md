# web/AGENTS.md — Frontend rules for agents

Instructions for any agent editing `web/`. Also read root `AGENTS.md` (especially `@kc-doc` documentation rules) and `web/src/ui/README.md`.

## 1. Use `src/ui/` primitives first

Import controls, forms, layout, feedback, and shells from `src/ui` (`web/src/ui/index.ts`; see `web/src/ui/README.md`) instead of writing raw `<button>`, `<input>`, `<select>`, `<kbd>`, or ad-hoc spacing/card CSS:

- **Actions**: `Button`, `IconButton` (requires `label`), `LinkButton`, `ConfirmDialog`
- **Forms**: `Field`, `Input`, `Textarea`, `Select`, `Checkbox`, `Toggle`
- **Layout & Type**: `Stack`, `Inline`, `Card`, `Text`, `SectionHeader`, `SettingRow`
- **Feedback & Status**: `Badge`, `Chip`, `StatusDot`, `Keycap`, `Callout`, `Spinner`, `Skeleton`
- **Shells & Icons**: `Overlay` (use `role`: `compact`, `standard`, `wide`, `tall`, `drawer`, `full`, `palette`), `Dialog`, `Tabs`, `TabPanel`, `EmptyState`, `ErrorBoundary`, `icons`
- **Variant Helpers**: `variants()`, `cn()`, `type VariantProps`

Always pass `testId` (`data-testid`) on interactive controls used in user workflows or browser verifiers.

## 2. Tokens, Tailwind v4, and static contracts

- `src/styles/tokens.css` is the **only** place raw colours, radii, spacing (`--space-*`), and font sizes (`--fs-*`) are defined.
- `src/styles/tailwind.css` resets every default Tailwind scale (`--color-*: initial`, `--spacing-*: initial`, `--radius-*: initial`, `--text-*: initial`, `--font-*: initial`, `--shadow-*: initial`) and maps karucode tokens via `@theme inline`.
- `src/styles/index.css` imports `tokens.css` and `tailwind.css` unlayered, then imports shared component sheets with `layer(components)` (`base.css` puts element resets in `@layer base`, which becomes `components.base` below utilities). Six component-owned sheets (`notifications.css`, `workbench-navigation.css`, and the four `control-*.css` files) are still unlayered and outrank utilities; restyle those with CSS classes until migrated.

Static checks fail CI on violations:

- `src/styles/ui-contract.test.ts` — forbids raw colours (`bg-[#...]`), non-geometry arbitrary brackets (geometry prefixes like `w-[...]`, `h-[...]`, `max-w-[...]`, `grid-cols-[...]` are allowed), `text-faint` (use `text-muted`), and default Tailwind palette classes (`bg-zinc-800`, `text-red-400`, …) in `src/**/*.{ts,tsx}`; verifies every mapped Tailwind token exists in `tokens.css`.
- `src/styles/scale.test.ts` — forbids raw colours, off-scale radii/font sizes/spacing, and undefined `var(--...)` tokens in `src/styles/*.css` outside `tokens.css`.
- `src/styles/layers.test.ts` — enforces `layer(components)` on `index.css` component imports and accounts for every stylesheet in `src/styles/`.
- `npm run lint` (`web/.oxlintrc.json`) — forbids `any` (`typescript/no-explicit-any`) and `<button>` without an explicit `type` (`react/button-has-type`).

## 3. Directory layout (post-Phase 2 decomposition)

```text
src/
  ui/              First-party Preact 11 UI primitives and variant helpers
  app/             App shell hooks & layout (WorkbenchLayout, GlobalOverlays,
                   useAppBoot, useGlobalKeybindings)
  features/        Decomposed feature modules:
                     access/     composer/     rail/     settings/     transcript/
  components/      Top-level feature views, git/, control-plane/, graduated shells
  state/           Signals store, eventReducer.ts, actions/ (barrel: actions.ts)
  api/             HTTP client, sharedStream.ts, endpoints/ (barrel: endpoints.ts),
                   types/ (barrel: types.ts)
  theme/           Theme engine, built-in palettes, colour math, storage
  lib/             Markdown/DOMPurify, diff, mentions, attachments, virtual window
  styles/          tokens.css, tailwind.css, index.css, component stylesheets
  test/            jsdom setup, fixtures, smoke tests
```

Prefer adding new domain logic/UI to the matching `src/features/<domain>/`, `src/state/actions/`, or `src/api/{types,endpoints}/` module rather than growing top-level monoliths. Keep the barrel re-exports (`api/types.ts`, `api/endpoints.ts`, `state/actions.ts`) intact for existing callers.

## 4. Verification commands

Run from `web/` before finishing any frontend change:

```bash
npm run typecheck
npm run lint
npm test
npm run audit
node scripts/design-audit.mjs
npm run build
```

To inspect every `src/ui/` primitive across themes, colour modes, and densities without starting the backend or a chat session, run `npm run dev` in `web/` and open `http://127.0.0.1:5173/?ui-gallery=1` (`src/ui/UiGallery.tsx`, loaded only when `import.meta.env.DEV` is true and excluded from `web/dist`).

When changing user-facing workspace behaviour, panels, shortcuts, or build/dev rules, update the `@kc-doc: web-ui` block in `web/src/app.tsx` and run from the repository root:

```bash
cargo run -p kc-docs -- --write
cargo run -p kc-docs -- --check
cargo test -p kc-docs
```

Never document an intention or URL/component that the code does not yet implement.
