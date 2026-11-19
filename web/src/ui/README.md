# `web/src/ui/` — First-Party UI Primitives

Typed, accessible Preact 11 primitives bound strictly to karucode's runtime design tokens (`src/styles/tokens.css` + `src/styles/tailwind.css`).

## Rules for Agents

1. **Import primitives from `src/ui` first** instead of authoring raw `<button>`, `<input>`, `<select>`, `<kbd>`, or bespoke spacing CSS.
2. **Never use Tailwind default palettes, `text-faint`, or arbitrary non-geometry brackets.** `src/styles/ui-contract.test.ts` fails CI on violations.
3. **Never write `any` or omit `type="button"`** on buttons (`npm run lint` / `.oxlintrc.json` enforces both).
4. **Always pass `testId`** on interactive controls used by user workflows or browser verifiers (`data-testid`).
5. **Use `Overlay` `role` names** (`compact`, `standard`, `wide`, `tall`, `drawer`, `full`, `palette`) or `ConfirmDialog` instead of custom dialog widths.

## Exports

- **Actions**: `Button`, `IconButton` (requires `label`), `LinkButton`, `ConfirmDialog`
- **Forms**: `Field` (auto-wires `htmlFor`, `aria-describedby`, `role="alert"` error), `Input`, `Textarea`, `Select`, `Checkbox`, `Toggle`
- **Layout & Type**: `Stack`, `Inline`, `Card`, `Text`, `SectionHeader`, `SettingRow`
- **Feedback & Status**: `Badge`, `Chip`, `StatusDot`, `Keycap`, `Callout`, `Spinner`, `Skeleton`
- **Shells**: `Overlay`, `Tabs`, `TabPanel`, `EmptyState`, `ErrorBoundary`, `icons`
- **Variant Helper**: `variants()`, `cn()`, `type VariantProps`
