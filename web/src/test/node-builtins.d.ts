/* Minimal typings for the Node built-ins the tests use.
 *
 * This package does not depend on `@types/node` (the app is browser-only), but a
 * few tests read repository files — the stylesheet scale contract, the first-paint
 * generator, the geometry/typography contracts. Declaring only what is used keeps
 * the runtime surface honest without pulling in the whole Node type set.
 */
declare module 'node:fs' {
  export function readFileSync(path: string | URL, encoding: 'utf8' | 'utf-8'): string;
  export function readdirSync(path: string | URL): string[];
  export function writeFileSync(path: string | URL, data: string): void;
}

/** Read by the env-gated code generators (`first-paint.emit.test.ts`). */
declare const process: {
  env: Record<string, string | undefined>;
  /** `RewindDialog.test.tsx` watches for a rejection nobody handled. */
  on(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
  off(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
};
