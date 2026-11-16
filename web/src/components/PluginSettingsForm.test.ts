import { describe, expect, it } from 'vitest';
import {
  changesFor,
  fieldProblem,
  type PluginSettingField,
} from './PluginSettingsForm';

const bool = (over: Partial<PluginSettingField> = {}): PluginSettingField => ({
  key: 'collapse',
  label: 'Collapse',
  type: 'bool',
  default: true,
  value: true,
  ...over,
});

describe('what the settings form sends', () => {
  it('sends only the fields that changed', () => {
    // A save must not rewrite every value: an unchanged field left alone keeps a
    // concurrent edit to it visible instead of silently reverting it.
    const fields: PluginSettingField[] = [
      bool(),
      { key: 'label', label: 'Label', type: 'text', default: 'Completed', value: 'Completed' },
    ];
    expect(changesFor(fields, { collapse: false, label: 'Completed' })).toEqual({
      collapse: false,
    });
    expect(changesFor(fields, { collapse: true, label: 'Completed' })).toEqual({});
  });

  it('never sends an empty secret, so opening the form cannot wipe one', () => {
    // The server never sends a secret's value back, so the field starts empty. If
    // an untouched empty field were sent, simply opening Settings would erase the
    // saved secret - the worst kind of destructive default.
    const fields: PluginSettingField[] = [
      { key: 'token', label: 'Token', type: 'secret', configured: true },
    ];
    expect(changesFor(fields, { token: '' })).toEqual({});
    expect(changesFor(fields, { token: 'new-value' })).toEqual({ token: 'new-value' });
  });

  it('accepts a number inside its range and refuses one outside', () => {
    const field: PluginSettingField = {
      key: 'after',
      label: 'After',
      type: 'number',
      default: 0,
      min: 0,
      max: 60,
      value: 0,
    };
    expect(fieldProblem(field, 0)).toBeNull();
    expect(fieldProblem(field, 60)).toBeNull();
    expect(fieldProblem(field, 61)).toContain('at most 60');
    expect(fieldProblem(field, -1)).toContain('at least 0');
    // An empty input parses to NaN; refusing it here is kinder than a round trip.
    expect(fieldProblem(field, Number.NaN)).toBe('Enter a number.');
  });

  it('refuses a select value that is not one of its options', () => {
    const field: PluginSettingField = {
      key: 'place',
      label: 'Place',
      type: 'select',
      default: 'bottom',
      value: 'bottom',
      options: [{ value: 'bottom' }, { value: 'top', label: 'Top' }],
    };
    expect(fieldProblem(field, 'top')).toBeNull();
    expect(fieldProblem(field, 'middle')).toBe('Choose one of the options.');
  });

  it('treats a stored value that differs from the default as a change', () => {
    // The form compares against what is in force, not against the default: a
    // value set by config.toml and left alone must not be "changed" by a save.
    const field = bool({ default: true, value: false });
    expect(changesFor([field], { collapse: false })).toEqual({});
    expect(changesFor([field], { collapse: true })).toEqual({ collapse: true });
  });
});
