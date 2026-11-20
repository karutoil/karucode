/* The touch, pointer and accessibility contract, enforced where it lives.
 *
 * Four of these rules are one declaration each in `base.css` plus one hook on a
 * primitive, and all four regress silently — a stylesheet has no
 * type checker, and a missing 44px floor looks exactly like a floor. So the
 * stylesheet is read here as text (the same way `styles/scale.test.ts` reads it)
 * and the primitive hooks get a DOM assertion.
 *
 * A11Y-04 is behaviour, not CSS, so the `Toggle` half of this file drives the
 * real component: the fix was making the row a `<label>` so the copy toggles the
 * switch — and *exactly once*, which is the part that is easy to get wrong.
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { useState } from 'preact/hooks';
import { Toggle } from '../components/Toggle';
import { Button, IconButton } from './Button';
import { Input, Select, Textarea } from './Input';

/* `import.meta.url` is an `http:` URL under jsdom, so the sheet is read relative
 * to the vitest root; `?raw` returns an empty string (vitest's `css: false`). */
function readSheet(name: string): string {
  const tried: string[] = [];
  for (const candidate of [`src/styles/${name}`, `web/src/styles/${name}`]) {
    try {
      return readFileSync(candidate, 'utf8');
    } catch {
      tried.push(candidate);
    }
  }
  throw new Error(`${name} not found; tried ${tried.join(', ')}`);
}

const base = readSheet('base.css');

/** The body of the first `@media (<query>) {` block, braces balanced. */
function mediaBlock(query: string): string {
  const start = base.search(new RegExp(`@media \\(${query.replace(/[()]/g, '\\$&')}\\) \\{`));
  expect(start, `base.css has an @media (${query}) block`).toBeGreaterThan(-1);
  let depth = 0;
  for (let i = base.indexOf('{', start); i < base.length; i += 1) {
    if (base[i] === '{') depth += 1;
    if (base[i] === '}') {
      depth -= 1;
      if (depth === 0) return base.slice(start, i + 1);
    }
  }
  throw new Error(`@media (${query}) is not closed`);
}

describe('A11Y-01 — one coarse-pointer size floor, on the primitives', () => {
  const coarse = mediaBlock('pointer: coarse');

  it.each(['.button', '.icon-button', '.ui-button', '.ui-icon-button', '.input', '.select', '.ui-field'])(
    'raises %s to 44×44',
    (selector) => {
      // The selector list is shared, so finding the name once proves the rule
      // covers it; the size assertion below pins the number.
      expect(coarse).toContain(selector);
    },
  );

  it('uses the 44px floor and keeps `sm` no smaller than the 40px house floor', () => {
    expect(coarse).toMatch(/min-width: 44px/);
    expect(coarse).toMatch(/min-height: 44px/);
    expect(coarse).toMatch(/min-height: 40px/);
  });

  it('never lets a small destructive or primary control under 44 on touch', () => {
    // §5.3: 40 is allowed only where a surface's density contract needs it, and
    // "never for a destructive or irreversible action".
    for (const variant of ['primary', 'danger', 'danger-ghost', 'stop']) {
      expect(coarse).toContain(`[data-size='sm'][data-variant='${variant}']`);
    }
  });

  it('grows the switch hit box without growing the 36×21 track', () => {
    // §8.8.1 measured the track as a 36×21 pill; a 44×44 box there is a blob.
    expect(coarse).toContain('.toggle::after');
    expect(coarse).toMatch(/\.toggle::after \{[^}]*width: 44px/);
    expect(coarse).toMatch(/\.toggle::after \{[^}]*height: 44px/);
  });

  it('emits the hooks the floor selects on', () => {
    render(
      <div>
        <Button testId="coarse-button">Save</Button>
        <IconButton label="Close" icon={<svg />} testId="coarse-icon" />
        <Input testId="coarse-input" />
        <Textarea testId="coarse-textarea" />
        <Select testId="coarse-select" />
      </div>,
    );

    expect(screen.getByTestId('coarse-button').classList.contains('ui-button')).toBe(true);
    expect(screen.getByTestId('coarse-icon').classList.contains('ui-icon-button')).toBe(true);
    for (const id of ['coarse-input', 'coarse-textarea', 'coarse-select']) {
      expect(screen.getByTestId(id).classList.contains('ui-field')).toBe(true);
    }
  });
});

describe('A11Y-08 — the iOS zoom floor is on the primitives', () => {
  it('sets 16px on a field inside the phone tier or on a coarse pointer', () => {
    const guard = mediaBlock('max-width: 640px), (pointer: coarse');
    for (const selector of ['.ui-field', '.input', '.palette-input']) {
      expect(guard).toContain(selector);
    }
    // `!important` because this sheet is in the `components` layer and the
    // primitives' `text-tiny`/`text-small` utilities sit above it — the whole
    // reason the old guard "passed" while every field still zoomed.
    expect(guard).toMatch(/font-size: var\(--fs-title\) !important/);
  });
});

describe('A11Y-07 — no double-tap zoom, no browser tap flash', () => {
  it('covers buttons, links, switches and the other tappable roles', () => {
    expect(base).toMatch(/touch-action: manipulation/);
    expect(base).toMatch(/-webkit-tap-highlight-color: transparent/);
    expect(base).toMatch(/\[role='switch'\]/);
    expect(base).toMatch(/\[role='option'\]/);
  });
});

describe('A11Y-09 — text size is the app\'s, not the browser\'s', () => {
  it('declares text-size-adjust on html', () => {
    const html = base.match(/\n\s*html \{([\s\S]*?)\n  \}/)?.[1] ?? '';
    expect(html).toMatch(/-webkit-text-size-adjust: 100%/);
    expect(html).toMatch(/\n\s*text-size-adjust: 100%/);
  });
});

describe('A11Y-04 — the toggle row toggles from its copy', () => {
  function Harness({
    initial = false,
    disabled = false,
    description,
  }: {
    initial?: boolean;
    disabled?: boolean;
    description?: string;
  }) {
    const [checked, setChecked] = useState(initial);
    return (
      <Toggle
        checked={checked}
        onChange={setChecked}
        label="Notify me"
        description={description}
        disabled={disabled}
        testId="notify-toggle"
      />
    );
  }

  it('labels the switch with the row, keeping the accessible name', () => {
    render(<Harness />);
    const control = screen.getByRole('switch', { name: 'Notify me' });
    const label = control.closest('label');

    expect(label).not.toBeNull();
    expect(label?.getAttribute('for')).toBe(control.id);
    expect(control.id).not.toBe('');
    expect(control.getAttribute('aria-checked')).toBe('false');
    expect(control.getAttribute('data-testid')).toBe('notify-toggle');
  });

  it('flips exactly once when the words are clicked', () => {
    render(<Harness />);
    const control = screen.getByRole('switch', { name: 'Notify me' });

    fireEvent.click(screen.getByText('Notify me'));

    expect(control.getAttribute('aria-checked')).toBe('true');
  });

  it('flips exactly once when the description is clicked', () => {
    const onChange = vi.fn();
    render(
      <Toggle
        checked={false}
        onChange={onChange}
        label="Notify me"
        description="Answers with a sound"
        testId="notify-toggle"
      />,
    );

    fireEvent.click(screen.getByText('Answers with a sound'));

    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it('still flips exactly once when the switch itself is clicked', () => {
    const onChange = vi.fn();
    render(<Toggle checked={false} onChange={onChange} label="Notify me" testId="notify-toggle" />);

    fireEvent.click(screen.getByRole('switch', { name: 'Notify me' }));

    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it('stays inert for the copy as well when disabled', () => {
    const onChange = vi.fn();
    render(<Toggle checked={false} onChange={onChange} label="Notify me" disabled testId="notify-toggle" />);

    fireEvent.click(screen.getByText('Notify me'));

    expect(onChange).not.toHaveBeenCalled();
  });

  it('keeps a caller-supplied id as the switch id', () => {
    render(<Toggle checked={false} onChange={() => {}} label="Advanced" id="advanced-toggle" />);
    const control = screen.getByRole('switch', { name: 'Advanced' });

    expect(control.id).toBe('advanced-toggle');
    expect(control.closest('label')?.getAttribute('for')).toBe('advanced-toggle');
  });
});
