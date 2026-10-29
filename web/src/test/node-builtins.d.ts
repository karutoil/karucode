/* Minimal typings for the Node built-ins the tests use.
 *
 * This package does not depend on `@types/node` (the app is browser-only), but
 * a few tests read repository files (for example the stylesheet geometry
 * contract in `ThemeStudio.test.tsx`). Declaring only what is used keeps the
 * runtime surface honest without pulling in the whole Node type set.
 */
declare module 'node:fs' {
  export function readFileSync(path: string | URL, encoding: 'utf8'): string;
}
