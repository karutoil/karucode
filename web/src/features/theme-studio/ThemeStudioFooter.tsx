/* The drawer's footer band: what is stored, how readable it is, and the actions.
 *
 * The band itself is the shell's `.overlay-footer`; this panel lays out what
 * sits in it. The four actions keep the drawer's `.theme-btn` hook because
 * `styles/theme-studio.test.ts` and the mock-fidelity suite both pin that class
 * on this row, while the behaviour and the disabled/primary/danger states come
 * from the `Button` primitive.
 */

import type { JSX, RefObject, TargetedEvent } from 'preact';
import { useId } from 'preact/hooks';
import {
  activeTheme,
  autosaveError,
  customThemes,
  draftEdited,
  themeContrast,
} from '../../theme';
import { Badge, Button, Inline, StatusDot, Text, icons } from '../../ui';

export interface ThemeStudioFooterProps {
  confirmDelete: boolean;
  confirmButton: RefObject<HTMLButtonElement | null>;
  onRequestDelete: () => void;
  onCancelDelete: () => void;
  onConfirmDelete: () => void;
  fileInput: RefObject<HTMLInputElement | null>;
  onPickImport: () => void;
  onImportFile: (event: TargetedEvent<HTMLInputElement, Event>) => Promise<void>;
  onExport: () => void;
  onOpenNaming: () => void;
}

export function ThemeStudioFooter({
  confirmDelete,
  confirmButton,
  onRequestDelete,
  onCancelDelete,
  onConfirmDelete,
  fileInput,
  onPickImport,
  onImportFile,
  onExport,
  onOpenNaming,
}: ThemeStudioFooterProps): JSX.Element {
  const deleteLabelId = useId();

  const theme = activeTheme.value;
  const contrast = themeContrast.value;
  const saveError = autosaveError.value;
  const dirty = draftEdited.value;
  const isCustom = customThemes.value.some((entry) => entry.id === theme.id);
  const activeLabel = dirty ? 'Unsaved theme' : theme.name;
  const low = contrast < 4.5;

  return (
    <footer class="overlay-footer theme-studio-foot">
      <div class="overlay-footer-actions theme-studio-foot-actions">
        <div class="theme-studio-status">
          <Text
            as="span"
            role="status"
            testId="theme-autosave"
            class="theme-autosave"
            size="tiny"
            tone="muted"
          >
            <StatusDot tone="ok" class="theme-dot" />
            {saveError ? (
              saveError
            ) : (
              <>
                Autosaved in this browser ·{' '}
                <strong data-testid="theme-current-name">{activeLabel}</strong>
              </>
            )}
          </Text>
          <Badge
            tone={low ? 'danger' : 'neutral'}
            class={low ? 'theme-contrast warning' : 'theme-contrast'}
            testId="theme-contrast"
          >
            Contrast {contrast.toFixed(1)}:1 {low ? '· Low' : '✓'}
          </Badge>
        </div>

        {confirmDelete ? (
          <div class="theme-delete-confirm" role="alertdialog" aria-labelledby={deleteLabelId}>
            <Text as="p" id={deleteLabelId} size="tiny">
              Delete “{theme.name}”? This removes it from this browser and cannot be undone.
            </Text>
            <Inline gap="4" class="theme-delete-actions">
              <Button
                buttonRef={confirmButton}
                variant="danger"
                class="theme-btn danger"
                onClick={onConfirmDelete}
                testId="theme-delete-confirm"
              >
                Delete theme
              </Button>
              <Button class="theme-btn" onClick={onCancelDelete}>
                Keep theme
              </Button>
            </Inline>
          </div>
        ) : null}

        <Inline gap="4" class="theme-foot-buttons">
          <Button
            class="theme-btn"
            icon={<icons.UploadIcon size={14} />}
            onClick={onPickImport}
            testId="theme-import"
          >
            Import
          </Button>
          <Button
            class="theme-btn"
            icon={<icons.DownloadIcon size={14} />}
            onClick={onExport}
            testId="theme-export"
          >
            Export
          </Button>
          <Button
            class="theme-btn"
            aria-label="Delete the current custom theme"
            title={isCustom ? 'Delete this custom theme' : 'Built-in themes cannot be deleted'}
            disabled={!isCustom}
            icon={<icons.TrashIcon size={14} />}
            onClick={onRequestDelete}
            testId="theme-delete"
          />
          <Button
            variant="primary"
            class="theme-btn primary spacer"
            onClick={onOpenNaming}
            testId="theme-save-as-new"
          >
            {dirty ? 'Save theme' : 'Save as new'}
          </Button>
        </Inline>

        <input
          ref={fileInput}
          class="theme-studio-import"
          type="file"
          accept="application/json,.json,.theme.json"
          aria-label="Import a theme JSON file"
          data-testid="theme-import-file"
          onChange={(event) => void onImportFile(event)}
        />
      </div>
    </footer>
  );
}
