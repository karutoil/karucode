import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import {
  ModelVariantsEditor,
  VARIANT_LEVELS,
  formatVariantBody,
  parseVariantRows,
  toVariantRows,
  variantLabel,
} from './ModelVariantsEditor';
import type { VariantRow, VariantsMap } from './ModelVariantsEditor';

/**
 * These tests pin the parts that make this editor 1:1 with config.toml: a body
 * is free-form JSON, `null` and `{}` stay different answers, and a level the UI
 * does not recognise is never silently deleted.
 */

const SAMPLE: VariantsMap = {
  none: { thinking: { type: 'disabled' } },
  high: { thinking: { type: 'enabled', budget_tokens: 32000 } },
};

describe('toVariantRows', () => {
  it('returns no rows for a model that declares no variants', () => {
    expect(toVariantRows(null)).toEqual([]);
    expect(toVariantRows(undefined)).toEqual([]);
  });

  it('returns no rows for a declared-but-empty map', () => {
    // `{}` means "offers nothing", which is a real state - it is not the same as
    // "not declared", and only the toggle distinguishes them.
    expect(toVariantRows({})).toEqual([]);
  });

  it('orders the known levels the way the composer does', () => {
    const rows = toVariantRows({ high: {}, none: {}, low: {} });
    expect(rows.map((row) => row.level)).toEqual(['none', 'low', 'high']);
  });

  it('keeps a level this build does not know instead of dropping it', () => {
    const rows = toVariantRows({ high: {}, turbo: { x: 1 } });
    expect(rows.map((row) => row.level)).toEqual(['high', 'turbo']);
  });

  it('pretty-prints the body so it is readable in the textarea', () => {
    const rows = toVariantRows(SAMPLE);
    expect(rows[1]?.body).toContain('\n');
    expect(JSON.parse(rows[1]!.body)).toEqual({
      thinking: { type: 'enabled', budget_tokens: 32000 },
    });
  });
});

describe('parseVariantRows', () => {
  const row = (level: string, body: string): VariantRow => ({ level, body });

  it('accepts the documented shapes', () => {
    const result = parseVariantRows([
      row('none', '{ "thinking": { "type": "disabled" } }'),
      row('high', '{ "thinking": { "type": "enabled", "budget_tokens": 32000 } }'),
      row('low', '{}'),
    ]);
    expect(result.errors).toEqual({});
    expect(result.rows).toEqual([
      { level: 'none', body: { thinking: { type: 'disabled' } } },
      { level: 'high', body: { thinking: { type: 'enabled', budget_tokens: 32000 } } },
      { level: 'low', body: {} },
    ]);
  });

  it('treats an empty body as an empty object, not an error', () => {
    expect(parseVariantRows([row('high', '')]).rows).toEqual([{ level: 'high', body: {} }]);
  });

  it('rejects malformed JSON with a message that names the problem', () => {
    const result = parseVariantRows([row('high', '{ oops')]);
    expect(result.rows).toEqual([]);
    expect(result.errors[0]).toMatch(/Not valid JSON/);
  });

  it('rejects a body that is not an object', () => {
    for (const body of ['[]', '"text"', '4', 'null']) {
      const result = parseVariantRows([row('high', body)]);
      expect(result.errors[0], body).toMatch(/must be a JSON object/);
    }
  });

  it('rejects a level the server would reject', () => {
    const result = parseVariantRows([row('extreme', '{}')]);
    expect(result.errors[0]).toMatch(/not a reasoning level/);
    expect(result.errors[0]).toContain('none');
  });

  it('rejects two rows claiming the same level', () => {
    const result = parseVariantRows([row('high', '{}'), row('high', '{"a":1}')]);
    expect(result.errors[1]).toMatch(/declared twice/);
  });

  it('rejects a body that would take over the request framing', () => {
    // The server refuses these too; catching them here keeps the message next
    // to the row instead of coming back as a 400.
    for (const key of ['model', 'messages', 'stream', 'tools']) {
      const result = parseVariantRows([row('high', JSON.stringify({ [key]: 1 }))]);
      expect(result.errors[0], key).toContain(key);
    }
  });

  it('allows nested objects and every value type inside a body', () => {
    const body = JSON.stringify({
      thinking: { type: 'enabled', budget_tokens: 4096, nested: { deep: [1, 2, 3] } },
      flag: true,
      ratio: 0.5,
      name: 'x',
    });
    expect(parseVariantRows([row('high', body)]).errors).toEqual({});
  });
});

describe('formatVariantBody', () => {
  it('renders an empty body as {}', () => {
    expect(formatVariantBody({})).toBe('{}');
    expect(formatVariantBody(undefined)).toBe('{}');
  });
});

describe('variantLabel', () => {
  it('uses the composer’s words for the known levels', () => {
    expect(VARIANT_LEVELS.map(variantLabel)).toEqual([
      'Off',
      'Minimal',
      'Low',
      'Medium',
      'High',
      'Max',
    ]);
  });

  it('falls back to the raw level for an unknown one', () => {
    expect(variantLabel('turbo')).toBe('turbo');
  });
});

describe('ModelVariantsEditor', () => {
  it('shows the toggle off and no rows when the model declares nothing', () => {
    render(<ModelVariantsEditor value={null} onChange={() => {}} />);
    const toggle = screen.getByRole('switch', { name: /Declare reasoning levels/ });
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    expect(screen.queryByRole('group', { name: /Declared reasoning levels/ })).toBeNull();
    // The hint has to say what "not declared" actually sends, because that is
    // the behaviour a reader is choosing between.
    expect(screen.getByText(/every level \(none, minimal, low, medium, high, max\)/)).not.toBeNull();
  });

  it('declares an empty map when the toggle is turned on', () => {
    const onChange = vi.fn();
    render(<ModelVariantsEditor value={null} onChange={onChange} />);
    fireEvent.click(screen.getByRole('switch', { name: /Declare reasoning levels/ }));
    // `{}`, not `null`: the model now declares variants and offers none.
    expect(onChange).toHaveBeenCalledWith({});
  });

  it('turns back into "not declared" when the toggle is turned off', () => {
    const onChange = vi.fn();
    render(<ModelVariantsEditor value={SAMPLE} onChange={onChange} />);
    fireEvent.click(screen.getByRole('switch', { name: /Declare reasoning levels/ }));
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it('renders one row per declared level with its body', () => {
    render(<ModelVariantsEditor value={SAMPLE} onChange={() => {}} />);
    const bodies = screen.getAllByLabelText(/Request body for level/) as HTMLTextAreaElement[];
    expect(bodies).toHaveLength(2);
    expect(bodies[1]?.value).toContain('budget_tokens');
    expect((screen.getByLabelText('Level 1') as HTMLSelectElement).value).toBe('none');
    expect((screen.getByLabelText('Level 2') as HTMLSelectElement).value).toBe('high');
  });

  it('says so when the map is declared but empty', () => {
    render(<ModelVariantsEditor value={{}} onChange={() => {}} />);
    expect(screen.getByText(/only “Default” applies/)).not.toBeNull();
  });

  it('reports a typed body back as parsed JSON', () => {
    const onChange = vi.fn();
    render(<ModelVariantsEditor value={{ high: {} }} onChange={onChange} />);
    fireEvent.input(screen.getByLabelText('Request body for level 1'), {
      target: { value: '{"thinking_budget": 8192}' },
    });
    expect(onChange).toHaveBeenLastCalledWith({ high: { thinking_budget: 8192 } });
  });

  it('never hands a malformed body up as a value', () => {
    const onChange = vi.fn();
    render(<ModelVariantsEditor value={{ high: {} }} onChange={onChange} />);
    fireEvent.input(screen.getByLabelText('Request body for level 1'), {
      target: { value: '{ nope' },
    });
    // The row does not parse, so it is not part of the map - and the dialog's
    // submit re-parses, so this cannot be saved either.
    for (const call of onChange.mock.calls) {
      expect(JSON.stringify(call[0])).not.toContain('nope');
    }
  });

  it('shows the parse error once the reader has touched the row', async () => {
    render(<ModelVariantsEditor value={{ high: {} }} onChange={() => {}} />);
    expect(screen.queryByRole('alert')).toBeNull();
    fireEvent.input(screen.getByLabelText('Request body for level 1'), {
      target: { value: '{ oops' },
    });
    // Preact re-renders synchronously here, so the alert is already present.
    expect(screen.getByRole('alert').textContent).toMatch(/Not valid JSON/);
  });

  it('removes a level', () => {
    const onChange = vi.fn();
    render(<ModelVariantsEditor value={SAMPLE} onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: 'Remove level 1' }));
    expect(onChange).toHaveBeenLastCalledWith({
      high: { thinking: { type: 'enabled', budget_tokens: 32000 } },
    });
  });

  it('adds a level that is not already declared', () => {
    const onChange = vi.fn();
    render(
      <ModelVariantsEditor value={{ none: {}, low: {}, medium: {}, high: {}, max: {} }} onChange={onChange} />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Add level' }));
    // The only free level is `minimal`.
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ minimal: {} }),
    );
  });

  it('stops offering add once every level is declared', () => {
    const full: VariantsMap = Object.fromEntries(VARIANT_LEVELS.map((level) => [level, {}]));
    render(<ModelVariantsEditor value={full} onChange={() => {}} />);
    expect((screen.getByRole('button', { name: 'Add level' }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  it('reports validity to the dialog so an unparsed body blocks the save', () => {
    const onValidityChange = vi.fn();
    render(
      <ModelVariantsEditor value={{ high: {} }} onChange={() => {}} onValidityChange={onValidityChange} />,
    );
    // Valid to start with.
    expect(onValidityChange).toHaveBeenLastCalledWith(true);
    fireEvent.input(screen.getByLabelText('Request body for level 1'), {
      target: { value: '{ not json' },
    });
    expect(onValidityChange).toHaveBeenLastCalledWith(false);
    // Typing a valid body again clears it.
    fireEvent.input(screen.getByLabelText('Request body for level 1'), {
      target: { value: '{"thinking_budget": 4096}' },
    });
    expect(onValidityChange).toHaveBeenLastCalledWith(true);
  });

  it('keeps an in-progress row visible instead of deleting it', () => {
    // A row whose body does not parse must not vanish: its level is kept with an
    // empty body, which is why the row (and the text in it) survives a re-render.
    render(<ModelVariantsEditor value={{ high: {} }} onChange={() => {}} />);
    fireEvent.input(screen.getByLabelText('Request body for level 1'), {
      target: { value: '{ not json' },
    });
    const body = screen.getByLabelText('Request body for level 1') as HTMLTextAreaElement;
    expect(body.value).toBe('{ not json');
    expect((screen.getByLabelText('Level 1') as HTMLSelectElement).value).toBe('high');
  });

  it('surfaces a save error from the server under the section', () => {
    render(
      <ModelVariantsEditor value={SAMPLE} onChange={() => {}} error="The server refused it" />,
    );
    expect(screen.getByText('The server refused it')).not.toBeNull();
  });

  it('disables every control while saving', () => {
    render(<ModelVariantsEditor value={SAMPLE} onChange={() => {}} disabled />);
    expect((screen.getByRole('switch') as HTMLButtonElement).disabled).toBe(true);
    for (const button of screen.getAllByRole('button')) {
      if (button.getAttribute('role') === 'switch') continue;
      expect((button as HTMLButtonElement).disabled).toBe(true);
    }
  });
});
