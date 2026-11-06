import { signal } from '@preact/signals';
import * as store from './store';

/** Transient Context drawer sections. These are deliberately not persisted. */
export type ContextSection = 'now' | 'usage' | 'access';

/** Whether the sibling Context overlay is open over the still-mounted results. */
export const contextOpen = signal(false);
export const contextSection = signal<ContextSection>('now');

/*
 * A phone Context overlay borrows the inspector sheet so the result underneath
 * stays mounted. Remember where the reader came from only for that open/close
 * pair: closing Context must not turn a conversation or the sessions sheet into
 * a newly opened result sheet. `undefined` means there is no phone snapshot;
 * `null` is a real snapshot (no sheet was open).
 */
let phoneSheetBeforeContext: store.MobileSheet | undefined;
let phoneSnapshotTaken = false;

/** Open Context without changing session, chat, terminal or result selection. */
export function openContext(section: ContextSection = 'now'): void {
  if (store.narrowViewport.value) {
    if (!phoneSnapshotTaken && store.mobileSheet.value !== 'inspector') {
      phoneSheetBeforeContext = store.mobileSheet.value;
      phoneSnapshotTaken = true;
    }
    store.mobileSheet.value = 'inspector';
  }
  contextSection.value = section;
  contextOpen.value = true;
}

/**
 * Close only Context. On a phone, restore the sheet that was underneath when the
 * overlay opened; result contents and their component-local state never unmount.
 */
export function closeContext(): void {
  const restorePhoneSheet =
    phoneSnapshotTaken && store.narrowViewport.value && store.mobileSheet.value === 'inspector';
  const previousSheet = phoneSheetBeforeContext ?? null;

  contextOpen.value = false;
  phoneSheetBeforeContext = undefined;
  phoneSnapshotTaken = false;

  // An external overlay/navigation action may already have selected another
  // sheet. Never overwrite that newer interaction with our stale snapshot.
  if (restorePhoneSheet) store.mobileSheet.value = previousSheet;
}

/** Toggle Context, retaining the selected section when it is reopened. */
export function toggleContext(): void {
  if (contextOpen.value) closeContext();
  else openContext(contextSection.value);
}

/** Change only the Context section; result selection is independent. */
export function selectContextSection(section: ContextSection): void {
  contextSection.value = section;
}
