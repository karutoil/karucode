import { signal } from '@preact/signals';

/** Control plane is a view over real sessions, never a second chat state. */
export const controlPlaneOpen = signal(false);
