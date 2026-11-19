import type { JSX } from 'preact';
import { useEffect, useId, useState } from 'preact/hooks';
import { Button, Callout, Select, Text, Textarea, Toggle } from '../ui';

/**
 * Editor for a model entry's `variants` map: which reasoning levels this model
 * offers, and the exact request body each one sends.
 *
 * This is the Settings-UI equivalent of the config file's
 *
 * ```toml
 * [providers.models.variants.high]
 * thinking = { type = "enabled", budget_tokens = 32000 }
 * ```
 *
 * and it is deliberately 1:1 with it. Anything the file can express, this can:
 * a body is free-form JSON, because every endpoint spells thinking differently
 * and the whole point of the feature is that the operator's payload is passed
 * through verbatim rather than squeezed into a fixed set of fields.
 *
 * The three states the map can be in, and how they are spelled here:
 *
 * * **absent** (`null`) - "not declared". Every built-in level is offered and
 *   sent as a bare `reasoning_effort: "<level>"`. This is what a model created
 *   before variants existed has, and leaving the section alone must keep it.
 * * **present and empty** (`{}`) - the model declares variants and offers none,
 *   so only "Default" applies.
 * * **present with entries** - exactly those levels are offered and accepted.
 *
 * `null` and `{}` are genuinely different answers, so "clear every row" must not
 * silently become "not declared"; the toggle below is what moves between them.
 */

/** The levels the server accepts, in the order the picker displays them. */
export const VARIANT_LEVELS = ['none', 'minimal', 'low', 'medium', 'high', 'max'] as const;

export type VariantLevel = (typeof VARIANT_LEVELS)[number];

/** One declared level and the raw JSON text of its body. */
export interface VariantRow {
  level: string;
  /** JSON text as typed; `{}` means an empty body. */
  body: string;
}

/** The variants map as the API carries it: level -> request-body keys. */
export type VariantsMap = Record<string, Record<string, unknown>>;

/**
 * Turn the wire map into editable rows, in the canonical level order.
 *
 * A level the UI does not know (a map written by a newer server, or a level the
 * server later adds) is still shown rather than dropped, in its own sorted
 * place after the known ones: silently deleting a level the operator configured
 * is exactly the failure this editor exists to prevent.
 */
export function toVariantRows(variants: VariantsMap | null | undefined): VariantRow[] {
  if (!variants) return [];
  const known = VARIANT_LEVELS.filter((level) => level in variants);
  const extra = Object.keys(variants)
    .filter((level) => !(VARIANT_LEVELS as readonly string[]).includes(level))
    .sort();
  return [...known, ...extra].map((level) => ({
    level,
    body: formatVariantBody(variants[level]),
  }));
}

/** Pretty-print a variant body so a textarea shows something a person reads. */
export function formatVariantBody(body: Record<string, unknown> | undefined): string {
  if (!body) return '{}';
  return JSON.stringify(body, null, 2);
}

/** A row as the server will receive it: the level, and its parsed body. */
export interface ParsedVariant {
  level: string;
  body: Record<string, unknown>;
}

export interface VariantParseResult {
  /** Parsed rows, in the order they were typed. */
  rows: ParsedVariant[];
  /** One message per problem, keyed by row index. Empty means valid. */
  errors: Record<number, string>;
}

/**
 * Validate and parse the rows into the map the API expects.
 *
 * Every rule the server enforces is checked here first, so a mistake is an
 * inline message instead of a rejected round-trip: a body must be a JSON
 * object, the level must be one of the six, a level may not be repeated, and a
 * variant may not set a framing key the provider owns (`model`, `messages`,
 * `stream`, `tools`). The server still validates - this is the friendly copy of
 * that rule, not a replacement for it.
 */
export function parseVariantRows(rows: readonly VariantRow[]): VariantParseResult {
  const errors: Record<number, string> = {};
  const parsed: ParsedVariant[] = [];
  const seen = new Set<string>();
  rows.forEach((row, index) => {
    const level = row.level.trim();
    if (!level) {
      errors[index] = 'Choose a level.';
      return;
    }
    if (!(VARIANT_LEVELS as readonly string[]).includes(level)) {
      errors[index] = `\`${level}\` is not a reasoning level; use one of ${VARIANT_LEVELS.join(', ')}.`;
      return;
    }
    if (seen.has(level)) {
      errors[index] = `\`${level}\` is declared twice; one body per level.`;
      return;
    }
    const text = row.body.trim();
    let value: unknown = {};
    if (text !== '') {
      try {
        value = JSON.parse(text);
      } catch (error) {
        errors[index] = `Not valid JSON: ${(error as Error).message}`;
        return;
      }
    }
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      errors[index] = 'The body must be a JSON object, e.g. {"thinking": {"budget_tokens": 4096}}.';
      return;
    }
    const reserved = ['model', 'messages', 'stream', 'tools'].filter((key) =>
      Object.prototype.hasOwnProperty.call(value, key),
    );
    if (reserved.length > 0) {
      errors[index] = `The provider owns \`${reserved.join('`, `')}\`; remove it from the body.`;
      return;
    }
    seen.add(level);
    parsed.push({ level, body: value as Record<string, unknown> });
  });
  return { rows: parsed, errors };
}

export interface ModelVariantsEditorProps {
  /**
   * The model's current map, `null` when it declares none, or `{}` when it
   * declares variants and offers no level.
   */
  value: VariantsMap | null;
  /**
   * Called on every edit with the new map - `null` for "not declared", `{}` for
   * "declared, empty", otherwise the parsed levels. The parent owns the value;
   * this component is controlled so the dialog's Save stays the only write.
   */
  onChange: (next: VariantsMap | null) => void;
  /**
   * Called with the rows that do not parse, so the dialog can refuse to save.
   *
   * It has to come from here rather than from `value`: an invalid body never
   * enters the map (which cannot hold it), so a dialog that only inspected
   * `value` would see a perfectly valid `{}` and save nothing. This is the
   * signal that says "there is text on screen you have not managed to parse".
   */
  onValidityChange?: (valid: boolean) => void;
  /** Problems to show under the section (a save the server rejected). */
  error?: string;
  disabled?: boolean;
}

/**
 * The variants section of the model-edit dialog.
 *
 * Controlled and stateless about the map itself: it derives rows from `value`
 * on every render, so the dialog's Save is the only thing that persists. Rows
 * keep a stable key by index because two rows may transiently share a level
 * while the reader is changing one to match another, and remounting the
 * textarea under the caret would lose a half-typed body.
 */
export function ModelVariantsEditor({
  value,
  onChange,
  onValidityChange,
  error,
  disabled = false,
}: ModelVariantsEditorProps): JSX.Element {
  const id = useId();
  const declared = value !== null;
  const rows = toVariantRows(value);
  // A body is parsed on every render, and the parent only ever receives rows
  // that parsed - so a half-typed body would be discarded on the next render
  // and the textarea would fight the reader. The draft holds the exact text
  // being edited, keyed by row index, and is discarded when the section is
  // switched off and on again.
  const [draft, setDraft] = useState<Record<number, string>>({});
  // Which rows the reader has actually edited, so a red message appears on the
  // row they are working on and not on one that arrived that way from a file.
  const [touched, setTouched] = useState<Record<number, boolean>>({});

  // The text each row currently shows, and whether it parses. Computed once
  // here rather than per row so the whole section agrees with itself.
  const shown = rows.map((row, index) => ({
    level: row.level,
    body: draft[index] ?? row.body,
  }));
  const { errors: rowErrors } = parseVariantRows(shown);
  const invalid = Object.keys(rowErrors).length > 0;

  // Tell the dialog whenever what is on screen stops being savable. Reporting
  // through an effect (rather than inside an input handler) keeps it true for
  // every path - typed text, a removed row, a level rename - and never calls
  // the parent during its own render.
  useEffect(() => {
    onValidityChange?.(!invalid);
  }, [invalid, onValidityChange]);

  // Switching the section (or opening the dialog on another model) retires the
  // drafts and flags with the rows they belonged to.
  useEffect(() => {
    setDraft({});
    setTouched({});
  }, [declared]);

  const commit = (next: VariantRow[]): void => {
    const result = parseVariantRows(next);
    // Only rows whose *body* parses can enter the map, because a map entry is a
    // JSON object by definition. A row that is still being typed keeps its level
    // with an empty body so it does not disappear mid-edit; the dialog refuses
    // to save while `onValidityChange` reports false, so the empty placeholder
    // can never be persisted by accident.
    const failed = new Set(Object.keys(result.errors).map(Number));
    const map: VariantsMap = {};
    next.forEach((row, index) => {
      const level = row.level.trim();
      if (!level) return;
      if (failed.has(index)) {
        if (!(level in map)) map[level] = {};
        return;
      }
      map[level] = result.rows.find((parsed) => parsed.level === level)?.body ?? {};
    });
    onChange(map);
  };

  const setRow = (index: number, patch: Partial<VariantRow>): void => {
    const next = rows.map((row, i) => (i === index ? { ...row, ...patch } : row));
    setTouched((active) => ({ ...active, [index]: true }));
    if (patch.body !== undefined) {
      // Keyed by row index, not by level: renaming a row must not lose the text
      // the reader typed into it, and two rows may briefly share a level.
      setDraft((drafts) => ({ ...drafts, [index]: patch.body as string }));
    }
    commit(next);
  };

  const addRow = (): void => {
    const used = new Set(rows.map((row) => row.level));
    const free = VARIANT_LEVELS.find((level) => !used.has(level)) ?? 'high';
    commit([...rows, { level: free, body: '{}' }]);
  };

  const removeRow = (index: number): void => {
    setTouched({});
    commit(rows.filter((_, i) => i !== index));
  };

  const toggleDeclared = (on: boolean): void => {
    setTouched({});
    onChange(on ? {} : null);
  };

  const hintId = `${id}-hint`;

  return (
    <div class="field field-wide model-variants">
      <div class="model-variants-head">
        <Text as="label" htmlFor={`${id}-toggle`} size="label" weight="semibold">
          Reasoning variants
        </Text>
        <Toggle
          id={`${id}-toggle`}
          checked={declared}
          disabled={disabled}
          onChange={toggleDeclared}
          label="Declare reasoning levels for this model"
        />
      </div>
      <Text as="p" size="tiny" tone="muted" class="model-variants-hint" id={hintId}>
        {declared
          ? 'Only these levels are offered in the composer and accepted on save. Each level sends exactly this body instead of a bare reasoning_effort string. A model that declares variants and offers none allows only “Default”.'
          : 'Not declared: every level (none, minimal, low, medium, high, max) is offered and sent as reasoning_effort = "<level>". Turn this on to send your own body per level — e.g. {"thinking": {"type": "enabled", "budget_tokens": 32000}}.'}
      </Text>
      {declared ? (
        <div class="model-variants-rows" role="group" aria-label="Declared reasoning levels">
          {rows.length === 0 ? (
            <Text as="p" size="small" tone="muted" class="model-variants-empty">
              No levels declared — only “Default” applies to this model.
            </Text>
          ) : null}
          {rows.map((row, index) => {
            // What the reader has typed wins over what the parent holds: an
            // unparsed body is never sent up, so the parent's copy of this row
            // is the last valid one.
            const bodyText = shown[index]?.body ?? row.body;
            const rowError = touched[index] ? rowErrors[index] : undefined;
            const bodyId = `${id}-body-${index}`;
            return (
              <div class="model-variant-row" key={index}>
                <div class="model-variant-head">
                  <Select
                    class="model-variant-level"
                    aria-label={`Level ${index + 1}`}
                    value={row.level}
                    disabled={disabled}
                    options={VARIANT_LEVELS.map((level) => ({
                      value: level,
                      label: variantLabel(level),
                    }))}
                    onChange={(event) => setRow(index, { level: event.currentTarget.value })}
                  />
                  <Button
                    size="sm"
                    class="model-variant-remove"
                    disabled={disabled}
                    aria-label={`Remove level ${index + 1}`}
                    onClick={() => removeRow(index)}
                  >
                    Remove
                  </Button>
                </div>
                <label class="sr-only" for={bodyId}>
                  Request body for level {index + 1}
                </label>
                <Textarea
                  id={bodyId}
                  class="model-variant-body"
                  rows={3}
                  mono
                  spellCheck={false}
                  value={bodyText}
                  disabled={disabled}
                  placeholder={'{\n  "thinking": { "type": "enabled", "budget_tokens": 4096 }\n}'}
                  onInput={(event) => setRow(index, { body: event.currentTarget.value })}
                />
                {rowError ? (
                  <Text as="p" size="tiny" tone="danger" role="alert" class="model-variant-error">
                    {rowError}
                  </Text>
                ) : null}
              </div>
            );
          })}
          <Button
            size="sm"
            class="model-variants-add"
            disabled={disabled || rows.length >= VARIANT_LEVELS.length}
            onClick={addRow}
          >
            Add level
          </Button>
        </div>
      ) : null}
      {error ? (
        <Callout tone="danger" role="alert" class="model-variants-error">
          {error}
        </Callout>
      ) : null}
    </div>
  );
}

/** The word shown in the level picker, matching the composer's own labels. */
export function variantLabel(level: string): string {
  switch (level) {
    case 'none':
      return 'Off';
    case 'minimal':
      return 'Minimal';
    case 'low':
      return 'Low';
    case 'medium':
      return 'Medium';
    case 'high':
      return 'High';
    case 'max':
      return 'Max';
    default:
      return level;
  }
}
