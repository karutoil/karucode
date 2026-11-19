import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import {
  BUILTIN_THEMES,
  DEFAULT_THEME,
  THEME_DENSITIES,
  applyTheme,
  findBuiltin,
  modePreset,
  type Theme,
  type ThemeDensity,
  type ThemeMode,
} from '../theme';
import {
  Badge,
  Button,
  Callout,
  Card,
  Checkbox,
  Chip,
  ConfirmDialog,
  EmptyState,
  Field,
  IconButton,
  Inline,
  Input,
  Keycap,
  LinkButton,
  SectionHeader,
  Select,
  SettingRow,
  Skeleton,
  Spinner,
  Stack,
  StatusDot,
  TabPanel,
  Tabs,
  Text,
  Textarea,
  Toggle,
  icons,
  type ButtonVariant,
  type CalloutTone,
  type ConfirmDialogTone,
  type FeedbackTone,
  type StatusDotTone,
  type SurfaceTone,
  type TabItem,
  type TextSize,
  type TextTone,
} from './index';

const BUTTON_VARIANTS: readonly ButtonVariant[] = [
  'secondary',
  'primary',
  'ghost',
  'danger',
  'danger-ghost',
  'stop',
];

const TEXT_SIZES: readonly TextSize[] = [
  'micro',
  'tiny',
  'small',
  'label',
  'message',
  'body',
  'lead',
  'title',
  'heading',
  'display',
  'hero',
];

const TEXT_TONES: readonly TextTone[] = [
  'default',
  'secondary',
  'muted',
  'accent',
  'success',
  'warn',
  'danger',
  'info',
];

const CARD_SURFACES: readonly Extract<SurfaceTone, 'surface' | 'raised' | 'inset' | 'sunken'>[] = [
  'surface',
  'raised',
  'inset',
  'sunken',
];

const FEEDBACK_TONES: readonly FeedbackTone[] = [
  'neutral',
  'accent',
  'info',
  'success',
  'warn',
  'danger',
];

const STATUS_DOT_TONES: readonly StatusDotTone[] = [
  'running',
  'ok',
  'error',
  'danger',
  'warn',
  'approval',
  'interrupted',
  'cancelled',
  'stopped',
  'muted',
  'idle',
  'tool',
  'streaming',
  'thinking',
];

const CALLOUT_TONES: readonly CalloutTone[] = ['info', 'warn', 'danger', 'success'];

interface GalleryChipItem {
  id: string;
  label: string;
  tone: FeedbackTone;
}

const INITIAL_CHIPS: readonly GalleryChipItem[] = [
  { id: 'rust', label: 'rust', tone: 'neutral' },
  { id: 'preact', label: 'preact', tone: 'accent' },
  { id: 'tailwind', label: 'tailwind', tone: 'info' },
  { id: 'vitest', label: 'vitest', tone: 'success' },
  { id: 'draft', label: 'draft', tone: 'warn' },
  { id: 'breaking', label: 'breaking', tone: 'danger' },
];

type GalleryTabId = 'overview' | 'tokens' | 'shortcuts';

const GALLERY_TABS: readonly TabItem<GalleryTabId>[] = [
  { id: 'overview', label: 'Overview', icon: <icons.SparkIcon size={14} /> },
  { id: 'tokens', label: 'Tokens', count: 18 },
  { id: 'shortcuts', label: 'Shortcuts', icon: <icons.KeyIcon size={14} /> },
];

function buildGalleryTheme(themeId: string, mode: ThemeMode, density: ThemeDensity): Theme {
  const base = findBuiltin(themeId) ?? DEFAULT_THEME;
  const colors =
    mode === base.mode
      ? base.colors
      : {
          ...modePreset(mode).colors,
          accent: base.colors.accent,
          syntaxKeyword: base.colors.accent,
        };
  return {
    ...base,
    mode,
    density,
    colors,
  };
}

function calloutIcon(tone: CalloutTone): JSX.Element {
  switch (tone) {
    case 'success':
      return <icons.CheckIcon size={15} />;
    case 'warn':
    case 'danger':
      return <icons.AlertIcon size={15} />;
    case 'info':
      return <icons.SparkIcon size={15} />;
  }
}

export function UiGallery(): JSX.Element {
  const [themeId, setThemeId] = useState<string>(DEFAULT_THEME.id);
  const [mode, setMode] = useState<ThemeMode>(DEFAULT_THEME.mode);
  const [density, setDensity] = useState<ThemeDensity>(DEFAULT_THEME.density);

  const [confirmTone, setConfirmTone] = useState<ConfirmDialogTone | null>(null);
  const [confirmStatus, setConfirmStatus] = useState<string>('none');

  const [workspaceName, setWorkspaceName] = useState<string>('karucode');
  const [endpointUrl, setEndpointUrl] = useState<string>('http://127.0.0.1:8787');
  const [tokenDraft, setTokenDraft] = useState<string>('invalid_token');
  const [systemPrompt, setSystemPrompt] = useState<string>(
    'Inspect changes before editing and verify with tests.',
  );
  const [providerProfile, setProviderProfile] = useState<string>('anthropic');
  const [autoScroll, setAutoScroll] = useState<boolean>(true);
  const [notificationsOn, setNotificationsOn] = useState<boolean>(false);

  const [streamMarkdown, setStreamMarkdown] = useState<boolean>(true);
  const [reasoningEffort, setReasoningEffort] = useState<string>('medium');

  const [chips, setChips] = useState<readonly GalleryChipItem[]>(INITIAL_CHIPS);
  const [activeTab, setActiveTab] = useState<GalleryTabId>('overview');

  useEffect(() => {
    applyTheme(buildGalleryTheme(themeId, mode, density));
  }, [themeId, mode, density]);

  const handleThemeChange = (nextId: string): void => {
    const next = findBuiltin(nextId) ?? DEFAULT_THEME;
    setThemeId(next.id);
    setMode(next.mode);
    applyTheme(buildGalleryTheme(next.id, next.mode, density));
  };

  const handleModeChange = (nextMode: ThemeMode): void => {
    setMode(nextMode);
    applyTheme(buildGalleryTheme(themeId, nextMode, density));
  };

  const handleDensityChange = (nextDensity: ThemeDensity): void => {
    setDensity(nextDensity);
    applyTheme(buildGalleryTheme(themeId, mode, nextDensity));
  };

  const handleCloseConfirm = (): void => {
    if (confirmTone) {
      setConfirmStatus(`cancelled (${confirmTone})`);
    }
    setConfirmTone(null);
  };

  const handleAcceptConfirm = (): void => {
    if (confirmTone) {
      setConfirmStatus(`confirmed (${confirmTone})`);
    }
    setConfirmTone(null);
  };

  return (
    <div data-testid="ui-gallery-root" class="h-full overflow-y-auto bg-bg text-text">
      <header class="sticky top-0 z-10 border-b border-line bg-rail px-6 py-4">
        <div class="mx-auto flex w-full max-w-[1200px] flex-wrap items-center justify-between gap-4">
          <Inline gap="3" align="center">
            <icons.PaletteIcon size={18} />
            <Text as="h1" size="title" weight="semibold">
              Karucode UI Gallery
            </Text>
            <Badge tone="accent">DEV</Badge>
          </Inline>

          <Inline gap="4" wrap align="center">
            <Inline gap="2" align="center">
              <Text as="label" htmlFor="gallery-theme-select" size="label" tone="secondary">
                Theme
              </Text>
              <Select
                id="gallery-theme-select"
                testId="gallery-theme-select"
                aria-label="Theme"
                class="w-[180px]"
                value={themeId}
                options={BUILTIN_THEMES.map((theme) => ({
                  value: theme.id,
                  label: `${theme.name} (${theme.mode})`,
                }))}
                onChange={(event) => handleThemeChange(event.currentTarget.value)}
              />
            </Inline>

            <Inline gap="2" role="group" aria-label="Color mode">
              <Button
                size="sm"
                variant={mode === 'dark' ? 'primary' : 'secondary'}
                aria-pressed={mode === 'dark'}
                testId="gallery-mode-dark"
                onClick={() => handleModeChange('dark')}
              >
                Dark
              </Button>
              <Button
                size="sm"
                variant={mode === 'light' ? 'primary' : 'secondary'}
                aria-pressed={mode === 'light'}
                testId="gallery-mode-light"
                onClick={() => handleModeChange('light')}
              >
                Light
              </Button>
            </Inline>

            <Inline gap="2" align="center">
              <Text as="label" htmlFor="gallery-density-select" size="label" tone="secondary">
                Density
              </Text>
              <Select
                id="gallery-density-select"
                testId="gallery-density-select"
                aria-label="Density"
                class="w-[150px]"
                value={density}
                options={THEME_DENSITIES.map((step) => ({
                  value: step,
                  label: step.charAt(0).toUpperCase() + step.slice(1),
                }))}
                onChange={(event) => handleDensityChange(event.currentTarget.value as ThemeDensity)}
              />
            </Inline>
          </Inline>
        </div>
      </header>

      <main class="mx-auto w-full max-w-[1200px] px-6 py-8">
        <Stack gap="8">
          <Card as="section" surface="surface" padding="6" testId="gallery-section-actions">
            <Stack gap="6">
              <SectionHeader
                eyebrow="01 · Actions"
                title="Buttons, links & confirmation"
                description="All six Button variants in md and sm sizes, loading/disabled states, IconButton, LinkButton, and an interactive ConfirmDialog."
                actions={<Badge tone="neutral">Button · ConfirmDialog</Badge>}
              />

              <Stack gap="4">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Medium buttons (6 variants)
                </Text>
                <Inline gap="3" wrap>
                  {BUTTON_VARIANTS.map((variant) => (
                    <Button
                      key={variant}
                      variant={variant}
                      size="md"
                      testId={`gallery-btn-md-${variant}`}
                    >
                      {variant}
                    </Button>
                  ))}
                </Inline>
              </Stack>

              <Stack gap="4">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Small buttons & states
                </Text>
                <Inline gap="3" wrap>
                  {BUTTON_VARIANTS.map((variant) => (
                    <Button
                      key={variant}
                      variant={variant}
                      size="sm"
                      testId={`gallery-btn-sm-${variant}`}
                    >
                      {variant}
                    </Button>
                  ))}
                  <Button
                    variant="primary"
                    size="md"
                    icon={<icons.PlusIcon size={14} />}
                    testId="gallery-btn-icon-left"
                  >
                    Leading icon
                  </Button>
                  <Button
                    variant="secondary"
                    size="md"
                    iconRight={<icons.ArrowRightIcon size={14} />}
                    testId="gallery-btn-icon-right"
                  >
                    Trailing icon
                  </Button>
                  <Button variant="primary" loading testId="gallery-btn-loading">
                    Loading
                  </Button>
                  <Button variant="secondary" disabled testId="gallery-btn-disabled">
                    Disabled
                  </Button>
                </Inline>
              </Stack>

              <Stack gap="4">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  IconButton & LinkButton
                </Text>
                <Inline gap="4" wrap align="center">
                  <IconButton
                    size="md"
                    label="Settings"
                    icon={<icons.SettingsIcon size={15} />}
                    testId="gallery-icon-btn-md"
                  />
                  <IconButton
                    size="sm"
                    label="Edit"
                    icon={<icons.EditIcon size={14} />}
                    testId="gallery-icon-btn-sm"
                  />
                  <IconButton
                    size="md"
                    label="Pinned"
                    pressed
                    icon={<icons.SparkIcon size={15} />}
                    testId="gallery-icon-btn-pressed"
                  />
                  <IconButton
                    size="md"
                    label="Disabled action"
                    disabled
                    icon={<icons.TrashIcon size={15} />}
                    testId="gallery-icon-btn-disabled"
                  />
                  <LinkButton tone="muted" testId="gallery-link-muted">
                    Muted link action
                  </LinkButton>
                  <LinkButton tone="accent" testId="gallery-link-accent">
                    Accent link action
                  </LinkButton>
                  <LinkButton tone="accent" disabled testId="gallery-link-disabled">
                    Disabled link
                  </LinkButton>
                </Inline>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Interactive ConfirmDialog
                </Text>
                <Inline gap="3" wrap align="center">
                  <Button
                    variant="primary"
                    testId="gallery-open-confirm-primary"
                    onClick={() => setConfirmTone('primary')}
                  >
                    Open primary dialog
                  </Button>
                  <Button
                    variant="danger-ghost"
                    testId="gallery-open-confirm-danger"
                    onClick={() => setConfirmTone('danger')}
                  >
                    Open danger dialog
                  </Button>
                  <Text size="small" tone="muted" testId="gallery-confirm-status">
                    Last outcome: {confirmStatus}
                  </Text>
                </Inline>
              </Stack>
            </Stack>
          </Card>

          <Card as="section" surface="surface" padding="6" testId="gallery-section-forms">
            <Stack gap="6">
              <SectionHeader
                eyebrow="02 · Forms"
                title="Fields, inputs & toggles"
                description="Field wiring with descriptions and errors, Input variants, Textarea, Select, Checkbox, and Toggle."
                actions={<Badge tone="neutral">Field · Input · Toggle</Badge>}
              />

              <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                <Field
                  label="Workspace name"
                  description="Displayed in the project rail."
                  required
                  testId="gallery-field-normal"
                >
                  {({ id, describedBy, invalid }) => (
                    <Input
                      id={id}
                      aria-describedby={describedBy}
                      invalid={invalid}
                      value={workspaceName}
                      placeholder="karucode"
                      testId="gallery-input-normal"
                      onInput={(event) => setWorkspaceName(event.currentTarget.value)}
                    />
                  )}
                </Field>

                <Field
                  label="Server endpoint"
                  description="Monospace input for URLs, paths, or commands."
                  testId="gallery-field-mono"
                >
                  {({ id, describedBy, invalid }) => (
                    <Input
                      id={id}
                      aria-describedby={describedBy}
                      invalid={invalid}
                      mono
                      value={endpointUrl}
                      placeholder="http://127.0.0.1:8787"
                      testId="gallery-input-mono"
                      onInput={(event) => setEndpointUrl(event.currentTarget.value)}
                    />
                  )}
                </Field>

                <Field
                  label="API token"
                  description="Must begin with the kc_ prefix."
                  error="Token format is invalid."
                  testId="gallery-field-invalid"
                >
                  {({ id, describedBy, invalid }) => (
                    <Input
                      id={id}
                      aria-describedby={describedBy}
                      invalid={invalid}
                      value={tokenDraft}
                      testId="gallery-input-invalid"
                      onInput={(event) => setTokenDraft(event.currentTarget.value)}
                    />
                  )}
                </Field>

                <Field
                  label="Locked identifier (sm + disabled)"
                  description="Small size variant in disabled state."
                  testId="gallery-field-disabled"
                >
                  {({ id, describedBy }) => (
                    <Input
                      id={id}
                      aria-describedby={describedBy}
                      size="sm"
                      disabled
                      value="proj_01J9X4K2"
                      testId="gallery-input-disabled"
                    />
                  )}
                </Field>

                <Field
                  label="System instructions"
                  description="Resizable multiline textarea."
                  testId="gallery-field-textarea"
                >
                  {({ id, describedBy, invalid }) => (
                    <Textarea
                      id={id}
                      aria-describedby={describedBy}
                      invalid={invalid}
                      rows={3}
                      value={systemPrompt}
                      testId="gallery-textarea-normal"
                      onInput={(event) => setSystemPrompt(event.currentTarget.value)}
                    />
                  )}
                </Field>

                <Field
                  label="JSON config (mono + invalid)"
                  error="Unexpected end of JSON input."
                  testId="gallery-field-textarea-invalid"
                >
                  {({ id, describedBy, invalid }) => (
                    <Textarea
                      id={id}
                      aria-describedby={describedBy}
                      invalid={invalid}
                      mono
                      rows={3}
                      defaultValue={'{\n  "port": 8787,'}
                      testId="gallery-textarea-invalid"
                    />
                  )}
                </Field>

                <Field
                  label="Provider profile"
                  description="Select from configured providers."
                  testId="gallery-field-select"
                >
                  {({ id, describedBy, invalid }) => (
                    <Select
                      id={id}
                      aria-describedby={describedBy}
                      invalid={invalid}
                      value={providerProfile}
                      testId="gallery-select-normal"
                      options={[
                        { value: 'anthropic', label: 'Anthropic (Claude)' },
                        { value: 'openai', label: 'OpenAI (GPT)' },
                        { value: 'local', label: 'Local Gateway', disabled: true },
                      ]}
                      onChange={(event) => setProviderProfile(event.currentTarget.value)}
                    />
                  )}
                </Field>

                <Field
                  label="Disabled invalid select"
                  error="No target environment selected."
                  testId="gallery-field-select-disabled"
                >
                  {({ id, describedBy, invalid }) => (
                    <Select
                      id={id}
                      aria-describedby={describedBy}
                      invalid={invalid}
                      disabled
                      value="none"
                      testId="gallery-select-disabled"
                      options={[{ value: 'none', label: 'Unavailable' }]}
                    />
                  )}
                </Field>
              </div>

              <Card surface="inset" padding="4">
                <Inline gap="8" wrap align="start">
                  <Checkbox
                    checked={autoScroll}
                    onChange={setAutoScroll}
                    label="Auto-scroll transcript"
                    description="Keep the newest output in view while streaming."
                    testId="gallery-checkbox"
                  />
                  <Checkbox
                    checked
                    disabled
                    onChange={() => undefined}
                    label="Strict contract guard (locked)"
                    description="Enforced by repository CI."
                    testId="gallery-checkbox-disabled"
                  />
                  <Toggle
                    checked={notificationsOn}
                    onChange={setNotificationsOn}
                    label="Desktop notifications"
                    description="Alert when an approval is requested."
                    testId="gallery-toggle"
                  />
                  <Toggle
                    checked={false}
                    disabled
                    onChange={() => undefined}
                    label="Disabled switch"
                    testId="gallery-toggle-disabled"
                  />
                </Inline>
              </Card>
            </Stack>
          </Card>

          <Card as="section" surface="surface" padding="6" testId="gallery-section-typography">
            <Stack gap="6">
              <SectionHeader
                eyebrow="03 · Typography & Surfaces"
                title="Type scale, tones, cards & setting rows"
                description="All 11 Text sizes from micro to hero, all 8 readable Text tones, Card surface rungs, and SettingRow."
                actions={<Badge tone="neutral">Text · Card · SettingRow</Badge>}
              />

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Type scale (micro → hero)
                </Text>
                <Card surface="inset" padding="4">
                  <Stack gap="3">
                    {TEXT_SIZES.map((size) => (
                      <Inline key={size} gap="4" align="baseline" wrap>
                        <Badge tone="neutral" class="w-[72px] justify-center">
                          {size}
                        </Badge>
                        <Text size={size} testId={`gallery-text-size-${size}`}>
                          The quick brown fox jumps over the lazy dog
                        </Text>
                      </Inline>
                    ))}
                  </Stack>
                </Card>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Readable text tones
                </Text>
                <Card surface="inset" padding="4">
                  <Inline gap="5" wrap>
                    {TEXT_TONES.map((tone) => (
                      <Text key={tone} size="body" tone={tone} weight="medium" testId={`gallery-text-tone-${tone}`}>
                        {tone}
                      </Text>
                    ))}
                  </Inline>
                </Card>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Card surface ladder
                </Text>
                <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
                  {CARD_SURFACES.map((surface) => (
                    <Card key={surface} surface={surface} padding="4" testId={`gallery-card-${surface}`}>
                      <Stack gap="2">
                        <Badge tone="accent">{surface}</Badge>
                        <Text size="small" tone="secondary">
                          Surface token <Text as="code" mono size="tiny">{`bg-${surface}`}</Text>
                        </Text>
                      </Stack>
                    </Card>
                  ))}
                </div>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  SettingRow
                </Text>
                <Card surface="surface" padding="4">
                  <SettingRow
                    title="Stream markdown incrementally"
                    description="Render partial assistant output as tokens arrive."
                    testId="gallery-setting-row-toggle"
                    control={
                      <Toggle
                        checked={streamMarkdown}
                        onChange={setStreamMarkdown}
                        label="Stream markdown incrementally"
                        testId="gallery-setting-toggle"
                      />
                    }
                  />
                  <SettingRow
                    title="Default reasoning effort"
                    description="Applied to new sessions unless overridden per chat."
                    htmlFor="gallery-setting-select"
                    testId="gallery-setting-row-select"
                    control={
                      <Select
                        id="gallery-setting-select"
                        class="w-[140px]"
                        value={reasoningEffort}
                        testId="gallery-setting-select"
                        options={[
                          { value: 'low', label: 'Low' },
                          { value: 'medium', label: 'Medium' },
                          { value: 'high', label: 'High' },
                        ]}
                        onChange={(event) => setReasoningEffort(event.currentTarget.value)}
                      />
                    }
                  />
                </Card>
              </Stack>
            </Stack>
          </Card>

          <Card as="section" surface="surface" padding="6" testId="gallery-section-feedback">
            <Stack gap="6">
              <SectionHeader
                eyebrow="04 · Feedback & Navigation"
                title="Badges, chips, status dots, callouts, tabs & empty state"
                description="Semantic status indicators, dismissible chips, keycaps, callouts, loading states, tabs, and EmptyState."
                actions={<Badge tone="neutral">Feedback · Tabs · EmptyState</Badge>}
              />

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Badges
                </Text>
                <Inline gap="3" wrap>
                  {FEEDBACK_TONES.map((tone) => (
                    <Badge key={tone} tone={tone} testId={`gallery-badge-${tone}`}>
                      {tone}
                    </Badge>
                  ))}
                  <Badge tone="accent" mono={false} testId="gallery-badge-sans">
                    Sans badge
                  </Badge>
                </Inline>
              </Stack>

              <Stack gap="3">
                <Inline justify="between" align="center" wrap>
                  <Text as="h4" size="label" tone="secondary" weight="semibold">
                    Dismissible Chips
                  </Text>
                  {chips.length < INITIAL_CHIPS.length ? (
                    <LinkButton
                      tone="accent"
                      testId="gallery-chips-reset"
                      onClick={() => setChips(INITIAL_CHIPS)}
                    >
                      Restore chips
                    </LinkButton>
                  ) : null}
                </Inline>
                <Inline gap="2" wrap>
                  {chips.map((chip) => (
                    <Chip
                      key={chip.id}
                      tone={chip.tone}
                      testId={`gallery-chip-${chip.id}`}
                      removeLabel={`Remove ${chip.label}`}
                      onRemove={() => setChips((prev) => prev.filter((item) => item.id !== chip.id))}
                    >
                      {chip.label}
                    </Chip>
                  ))}
                </Inline>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  StatusDot (all 14 tones)
                </Text>
                <div class="grid grid-cols-2 sm:grid-cols-4 md:grid-cols-7 gap-3">
                  {STATUS_DOT_TONES.map((tone) => (
                    <Inline key={tone} gap="2" align="center">
                      <StatusDot tone={tone} label={`Status ${tone}`} testId={`gallery-dot-${tone}`} />
                      <Text size="tiny" mono tone="secondary">
                        {tone}
                      </Text>
                    </Inline>
                  ))}
                </div>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Keycap, Spinner & Skeleton
                </Text>
                <Inline gap="6" wrap align="center">
                  <Inline gap="1" align="center">
                    <Keycap>Ctrl</Keycap>
                    <Text size="tiny" tone="muted">+</Text>
                    <Keycap>Shift</Keycap>
                    <Text size="tiny" tone="muted">+</Text>
                    <Keycap>L</Keycap>
                  </Inline>
                  <Inline gap="3" align="center">
                    <Spinner size={14} label="Loading small" testId="gallery-spinner-sm" />
                    <Spinner size={20} label="Loading medium" testId="gallery-spinner-md" />
                  </Inline>
                  <Stack gap="2">
                    <Skeleton class="h-[14px] w-[180px]" label="Loading heading" testId="gallery-skeleton-1" />
                    <Skeleton class="h-[12px] w-[260px]" testId="gallery-skeleton-2" />
                  </Stack>
                </Inline>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Callouts (4 tones)
                </Text>
                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                  {CALLOUT_TONES.map((tone) => (
                    <Callout
                      key={tone}
                      tone={tone}
                      title={`${tone.toUpperCase()} notice`}
                      icon={calloutIcon(tone)}
                      testId={`gallery-callout-${tone}`}
                      actions={
                        tone === 'warn' ? (
                          <Button size="sm" variant="secondary">
                            Review
                          </Button>
                        ) : undefined
                      }
                    >
                      Semantic callout message using the {tone} token palette.
                    </Callout>
                  ))}
                </div>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  Tabs & TabPanel
                </Text>
                <Card surface="inset" padding="4">
                  <Stack gap="4">
                    <Tabs
                      items={GALLERY_TABS}
                      active={activeTab}
                      onChange={setActiveTab}
                      label="Gallery sample tabs"
                      idPrefix="gallery-tabs"
                    />
                    <TabPanel id="overview" idPrefix="gallery-tabs" active={activeTab === 'overview'}>
                      <Text size="small" tone="secondary" testId="gallery-tabpanel-overview">
                        Overview panel: primitives compose with shared spacing and surface tokens.
                      </Text>
                    </TabPanel>
                    <TabPanel id="tokens" idPrefix="gallery-tabs" active={activeTab === 'tokens'}>
                      <Text size="small" tone="secondary" testId="gallery-tabpanel-tokens">
                        Tokens panel: 18 editable theme colours drive every semantic utility.
                      </Text>
                    </TabPanel>
                    <TabPanel id="shortcuts" idPrefix="gallery-tabs" active={activeTab === 'shortcuts'}>
                      <Inline gap="2" align="center" testId="gallery-tabpanel-shortcuts">
                        <Text size="small" tone="secondary">
                          Open Theme Studio:
                        </Text>
                        <Keycap>Ctrl</Keycap>
                        <Keycap>Shift</Keycap>
                        <Keycap>L</Keycap>
                      </Inline>
                    </TabPanel>
                  </Stack>
                </Card>
              </Stack>

              <Stack gap="3">
                <Text as="h4" size="label" tone="secondary" weight="semibold">
                  EmptyState
                </Text>
                <Card surface="inset" padding="4" testId="gallery-empty-state-card">
                  <EmptyState />
                </Card>
              </Stack>
            </Stack>
          </Card>
        </Stack>
      </main>

      <ConfirmDialog
        open={confirmTone !== null}
        tone={confirmTone ?? 'primary'}
        title={confirmTone === 'danger' ? 'Reset theme overrides?' : 'Apply gallery preset?'}
        description={
          confirmTone === 'danger'
            ? 'This demonstrates the destructive confirmation dialog tone.'
            : 'This demonstrates the primary confirmation dialog tone.'
        }
        confirmLabel={confirmTone === 'danger' ? 'Reset' : 'Apply'}
        cancelLabel="Cancel"
        footerNote="Press Escape or click the backdrop to dismiss."
        testId="gallery-confirm-dialog"
        onClose={handleCloseConfirm}
        onConfirm={handleAcceptConfirm}
      >
        <Text size="small" tone="secondary">
          Dialog body rendered inside <Text as="code" mono size="small">ConfirmDialog</Text>.
        </Text>
      </ConfirmDialog>
    </div>
  );
}
