/* Theme Studio drawer.
 *
 * Mounted once by `app.tsx`; visibility is driven by `store.themeStudioOpen`,
 * opened via `actions.openThemeStudio()` from the shell header/rail and the
 * `Ctrl/Cmd+Shift+L` shortcut (owned by `app.tsx`, next to the other global
 * shortcuts, so the drawer does not race a second listener).
 *
 * Design reference: `karucode-v3-themes.html`. Unlike the mock, nothing here
 * uses `window.prompt`/`window.confirm`: naming is an inline form and deleting
 * is an inline confirmation, so the flow stays keyboard- and screen-reader-
 * friendly (and testable). Both of those, and the file operations, live in
 * `features/theme-studio/useThemeStudioController.ts`; this file is the drawer
 * shell — the layer, the veil, the header, the scroller and the footer band —
 * and the five panels inside it are one file each under
 * `features/theme-studio/`.
 *
 * Every control inside the panels is a `src/ui/` primitive except the three
 * elements whose look *is* their geometry and is pinned by
 * `styles/theme-studio.test.ts`: the preset card (`Button`'s fixed height would
 * flatten the 62px miniature), the swatch's native colour input (drawn as an
 * invisible overlay of the swatch) and the hidden file input.
 */

import type { JSX } from 'preact';
import { createPortal } from 'preact/compat';
import { useId, useRef } from 'preact/hooks';
import { ThemeAdvancedSection } from '../features/theme-studio/ThemeAdvancedSection';
import { ThemeColorsSection } from '../features/theme-studio/ThemeColorsSection';
import { ThemeInterfaceSection } from '../features/theme-studio/ThemeInterfaceSection';
import { ThemePresetSection } from '../features/theme-studio/ThemePresetSection';
import { ThemeStudioFooter } from '../features/theme-studio/ThemeStudioFooter';
import { ThemeTranscriptSection } from '../features/theme-studio/ThemeTranscriptSection';
import { useThemeStudioController } from '../features/theme-studio/useThemeStudioController';
import { useFocusTrap } from '../lib/dom';
import * as actions from '../state/actions';
import * as store from '../state/store';
import { IconButton, icons } from '../ui';

export function ThemeStudio(): JSX.Element | null {
  const open = store.themeStudioOpen.value;

  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const subtitleId = useId();

  const studio = useThemeStudioController({ open, panel: panelRef });

  useFocusTrap(panelRef, open, actions.closeThemeStudio);

  // Escape is the trap's job (`lib/dom.ts` intercepts it in the capture phase on
  // `document`, defers to a `[data-escape-local]` control and closes only the
  // topmost surface). A window-level fallback used to sit here; it could never
  // fire for a key pressed inside the drawer — the trap stopPropagation's it
  // first — and if it had, it would have closed the drawer underneath the delete
  // confirmation instead of the confirmation. Removed rather than re-gated: the
  // trap already covers the one case it claimed to.

  if (!open) return null;

  const layer = (
    <div class="dialog-layer" data-overlay-role="drawer">
      <div class="dialog-backdrop" aria-hidden="true" onClick={actions.closeThemeStudio} />
      <div
        class="overlay overlay-drawer theme-studio"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={subtitleId}
        ref={panelRef}
        data-testid="theme-studio"
        data-role="drawer"
      >
        <header class="overlay-header">
          <div class="overlay-headings theme-studio-head">
            <h2 class="overlay-title theme-studio-title" id={titleId}>
              Theme studio
            </h2>
            <p class="overlay-description theme-studio-subtitle" id={subtitleId}>
              Make the whole workspace feel like yours.
            </p>
          </div>
          <IconButton
            label="Close theme studio"
            icon={<icons.CloseIcon size={14} />}
            class="overlay-close"
            onClick={actions.closeThemeStudio}
          />
        </header>

        <div class="theme-studio-scroll">
          <ThemePresetSection
            naming={studio.naming}
            nameValue={studio.nameValue}
            nameInput={studio.nameInput}
            onNameInput={studio.setNameValue}
            onOpenNaming={studio.openNaming}
            onCancelNaming={studio.cancelNaming}
            onDismissNaming={studio.dismissNaming}
            onSubmitName={studio.submitName}
          />
          <ThemeColorsSection
            hexEdits={studio.hexEdits}
            onHexInput={studio.onHexInput}
            onHexBlur={studio.onHexBlur}
          />
          <ThemeInterfaceSection />
          <ThemeTranscriptSection />
          <ThemeAdvancedSection copyVariables={studio.copyVariables} />
        </div>

        <ThemeStudioFooter
          confirmDelete={studio.confirmDelete}
          confirmButton={studio.confirmButton}
          onRequestDelete={studio.requestDelete}
          onCancelDelete={studio.cancelDelete}
          onConfirmDelete={studio.confirmDeleteNow}
          fileInput={studio.fileInput}
          onPickImport={studio.pickImportFile}
          onImportFile={studio.onImportFile}
          onExport={studio.exportTheme}
          onOpenNaming={studio.openNaming}
        />
      </div>
    </div>
  );

  // Portal into <body> like `Dialog` does. `.app-root` is a stacking context
  // (`isolation: isolate`), so `z-index: 110` on the layer was confined inside
  // it and never beat a portalled dialog at 100 — Settings → Theme studio →
  // Ctrl+K left two `aria-modal="true"` surfaces alive, the older one buried
  // under the newer dialog's scrim (DESIGN-AUDIT overlays B6). `document` is
  // absent in a server render and the node-environment suites.
  if (typeof document === 'undefined') return layer;
  return createPortal(layer, document.body);
}
