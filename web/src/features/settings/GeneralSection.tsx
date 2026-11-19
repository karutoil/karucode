import type { JSX } from 'preact';
import { PaletteIcon } from '../../components/icons';
import { SettingsAdvanced } from '../../components/SettingsAdvanced';
import * as actions from '../../state/actions';
import * as store from '../../state/store';
import * as THEME from '../../theme';
import type {
  ThemeDensity,
  ThemeFont,
  ThemeGrouping,
  ThemeMode,
  ThemeMono,
  ThemeThinking,
  ThemeToolStyle,
  ThemeWorking,
} from '../../theme';
import { Button, Select, SettingRow } from '../../ui';
import { DENSITY_LABELS } from './formModel';
import { Field } from './SharedControls';

export interface GeneralSectionProps {
  advancedOpen: boolean;
  onAdvancedChange: (open: boolean) => void;
}

export function GeneralSection({ advancedOpen, onAdvancedChange }: GeneralSectionProps): JSX.Element {
  return (
    <>
      <section class="settings-section" aria-labelledby="settings-appearance">
        <div class="settings-section-head">
          <h3 id="settings-appearance">Theme and layout</h3>
          <span class="settings-section-note">Applied as you change it</span>
        </div>
        <SettingRow
          class="setting-row"
          title="Theme studio"
          description="Pick a built-in theme or tune every surface, accent, font and density."
          control={
            <Button
              variant="secondary"
              icon={<PaletteIcon size={14} />}
              class="border-accent-line bg-accent-bg text-accent-text"
              onClick={actions.openThemeStudio}
            >
              Theme studio
            </Button>
          }
        />
        <div class="field-grid">
          <Field
            id="settings-theme"
            label="Theme"
            hint="Applies immediately. Saving this sheet is not required."
          >
            <Select
              id="settings-theme"
              aria-describedby="settings-theme-hint"
              value={THEME.activeThemeId.value}
              onChange={(event) => THEME.selectTheme(event.currentTarget.value)}
            >
              {THEME.themeLibrary.value.map((entry) => (
                <option value={entry.id} key={entry.id}>
                  {entry.name}
                </option>
              ))}
              {THEME.draftEdited.value ? (
                <option value="draft">{THEME.activeThemeName.value}</option>
              ) : null}
            </Select>
          </Field>
          <Field id="settings-density" label="Density" hint="Moves type size and spacing together.">
            <Select
              id="settings-density"
              aria-describedby="settings-density-hint"
              value={THEME.activeTheme.value.density}
              onChange={(event) =>
                THEME.setDensity(event.currentTarget.value as ThemeDensity)
              }
              options={THEME.THEME_DENSITIES.map((density) => ({
                value: density,
                label: DENSITY_LABELS[density],
              }))}
            />
          </Field>
          <Field id="settings-mode" label="Colour mode" hint="Re-bases the surfaces, keeping your accent.">
            <Select
              id="settings-mode"
              aria-describedby="settings-mode-hint"
              value={THEME.activeTheme.value.mode}
              onChange={(event) => THEME.setThemeMode(event.currentTarget.value as ThemeMode)}
              options={THEME.THEME_MODES.map((mode) => ({
                value: mode,
                label: mode === 'dark' ? 'Dark' : 'Light',
              }))}
            />
          </Field>
          <Field
            id="settings-results-mode"
            label="Results panel"
            hint={
              store.inspectorMode.value === 'popout'
                ? 'Pop out: the conversation keeps the full width and Results opens over it.'
                : 'Docked: Results keeps its own column beside the conversation.'
            }
          >
            <Select
              id="settings-results-mode"
              aria-describedby="settings-results-mode-hint"
              value={store.inspectorMode.value}
              onChange={(event) =>
                actions.setInspectorMode(event.currentTarget.value as store.InspectorMode)
              }
              options={[
                { value: 'docked', label: 'Docked' },
                { value: 'popout', label: 'Pop out' },
              ]}
            />
          </Field>
          <Field
            id="settings-project-dropdown"
            label="Project dropdown"
            hint={
              store.projectMenuOpen.value === 'click'
                ? 'Click: the project name opens the list when you press it, and the pointer may cross it freely.'
                : 'Hover: resting on the project name opens the list of projects.'
            }
          >
            <Select
              id="settings-project-dropdown"
              aria-describedby="settings-project-dropdown-hint"
              value={store.projectMenuOpen.value}
              onChange={(event) =>
                actions.setProjectMenuOpen(event.currentTarget.value as store.ProjectMenuOpen)
              }
              options={[
                { value: 'hover', label: 'Open on hover' },
                { value: 'click', label: 'Open on click' },
              ]}
            />
          </Field>
          <Field
            id="settings-send-key"
            label="Composer send key"
            hint={
              store.sendKey.value === 'enter'
                ? 'Enter sends; Shift+Enter or Ctrl/Cmd+Enter starts a new line.'
                : 'Ctrl/Cmd+Enter sends; Enter starts a new line.'
            }
          >
            <Select
              id="settings-send-key"
              aria-describedby="settings-send-key-hint"
              value={store.sendKey.value}
              onChange={(event) =>
                actions.setSendKey(event.currentTarget.value as store.SendKey)
              }
              options={[
                { value: 'modifier-enter', label: 'Ctrl/Cmd + Enter sends' },
                { value: 'enter', label: 'Enter sends' },
              ]}
            />
          </Field>
        </div>
      </section>

      <section class="settings-section" aria-labelledby="settings-transcript">
        <div class="settings-section-head">
          <h3 id="settings-transcript">Transcript</h3>
          <span class="settings-section-note">
            How the conversation is drawn; the rest of the theme lives in the Studio
          </span>
        </div>
        <div class="field-grid">
          <Field id="settings-tool-style" label="Tool calls">
            <Select
              id="settings-tool-style"
              value={THEME.activeTheme.value.toolStyle}
              onChange={(event) =>
                THEME.setToolStyle(event.currentTarget.value as ThemeToolStyle)
              }
              options={THEME.THEME_TOOL_STYLE_CHOICES.map(({ value, label }) => ({ value, label }))}
            />
          </Field>
          <Field id="settings-grouping" label="Grouping">
            <Select
              id="settings-grouping"
              value={THEME.activeTheme.value.grouping}
              onChange={(event) =>
                THEME.setGrouping(event.currentTarget.value as ThemeGrouping)
              }
              options={THEME.THEME_GROUPING_CHOICES.map(({ value, label }) => ({ value, label }))}
            />
          </Field>
          <Field id="settings-working" label="Working steps">
            <Select
              id="settings-working"
              value={THEME.activeTheme.value.working}
              onChange={(event) =>
                THEME.setWorking(event.currentTarget.value as ThemeWorking)
              }
              options={THEME.THEME_WORKING_CHOICES.map(({ value, label }) => ({ value, label }))}
            />
          </Field>
          <Field id="settings-thinking" label="Reasoning blocks">
            <Select
              id="settings-thinking"
              value={THEME.activeTheme.value.thinking}
              onChange={(event) =>
                THEME.setThinking(event.currentTarget.value as ThemeThinking)
              }
              options={THEME.THEME_THINKING_CHOICES.map(({ value, label }) => ({ value, label }))}
            />
          </Field>
        </div>
      </section>

      <SettingsAdvanced
        id="settings-general-advanced"
        open={advancedOpen}
        onOpenChange={onAdvancedChange}
        note="Fonts, rounding and the transcript layout"
      >
        <section class="settings-section" aria-labelledby="settings-type">
          <div class="settings-section-head">
            <h3 id="settings-type">Type and shape</h3>
            <span class="settings-section-note">Every change is applied and saved at once</span>
          </div>
          <div class="field-grid">
            <Field id="settings-font" label="Interface font">
              <Select
                id="settings-font"
                value={THEME.activeTheme.value.font}
                onChange={(event) => THEME.setFont(event.currentTarget.value as ThemeFont)}
                options={THEME.THEME_FONTS.map((font) => ({
                  value: font,
                  label: THEME.THEME_FONT_LABELS[font],
                }))}
              />
            </Field>
            <Field id="settings-mono" label="Code font">
              <Select
                id="settings-mono"
                value={THEME.activeTheme.value.mono}
                onChange={(event) => THEME.setMono(event.currentTarget.value as ThemeMono)}
                options={THEME.THEME_MONO_FONTS.map((mono) => ({
                  value: mono,
                  label: THEME.THEME_MONO_LABELS[mono],
                }))}
              />
            </Field>
            <Field
              id="settings-rounding"
              label="Corner rounding (px)"
              hint="0 is square, 22 is fully rounded."
              wide
            >
              <input
                id="settings-rounding"
                class="input-range"
                type="range"
                min="0"
                max="22"
                step="1"
                value={String(THEME.activeTheme.value.rounding)}
                onInput={(event) => THEME.setRounding(Number(event.currentTarget.value))}
              />
            </Field>
          </div>
        </section>
      </SettingsAdvanced>
    </>
  );
}
