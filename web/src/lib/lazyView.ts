import type { ComponentType } from 'preact';
import { lazy } from 'preact/compat';

/**
 * `lazy()` for a module that exports its component by name.
 *
 * Preact's `lazy` hands whatever the loader resolved straight to
 * `createElement` unless the module has a `default` export — and a module
 * namespace object is not a component, so `lazy(() => import('./Thing'))`
 * renders nothing useful. Every view in this app exports by name, so the
 * reshaping into `{ default }` happens once here instead of in a `.then()` at
 * each call site.
 *
 * The loader runs on the first render of the returned component; `lazy` caches
 * the module, so the second open of a panel that has already been visited
 * mounts synchronously. Wrap the component in `<Suspense>`: its fallback is
 * what renders while the chunk is in flight.
 *
 * Splitting on these boundaries is about the *first* paint: a view that is
 * behind a keybinding, a tab or a sign-in state does not belong in the chunk
 * the reader has to download to see the chat.
 */
export function lazyNamed<P = Record<string, never>>(
  loader: () => Promise<Record<string, unknown>>,
  name: string,
): ComponentType<P> {
  return lazy(() => loader().then((module) => ({ default: module[name] as ComponentType<P> })));
}
