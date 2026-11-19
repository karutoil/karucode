import { fireEvent, render, screen } from '@testing-library/preact';
import { describe, expect, it, vi } from 'vitest';
import { Card, Inline, SectionHeader, SettingRow, Stack, Text } from './Layout';
import type { SpaceStep, SurfaceTone, TextSize, TextTone } from './types';

describe('Stack', () => {
  it('renders a vertical flex container with default gap-4', () => {
    render(
      <Stack testId="stack">
        <span>one</span>
        <span>two</span>
      </Stack>,
    );
    const el = screen.getByTestId('stack');
    expect(el.tagName).toBe('DIV');
    expect(el.className).toContain('flex');
    expect(el.className).toContain('flex-col');
    expect(el.className).toContain('gap-4');
    expect(el.textContent).toBe('onetwo');
  });

  it('supports custom gap, align, justify, id, aria-label, and extra classes', () => {
    render(
      <Stack
        id="my-stack"
        testId="stack"
        gap="7"
        align="center"
        justify="between"
        aria-label="Settings list"
        class="p-2"
      >
        <span>item</span>
      </Stack>,
    );
    const el = screen.getByTestId('stack');
    expect(el.id).toBe('my-stack');
    expect(el.getAttribute('aria-label')).toBe('Settings list');
    expect(el.className).toContain('gap-7');
    expect(el.className).toContain('items-center');
    expect(el.className).toContain('justify-between');
    expect(el.className).toContain('p-2');
  });

  it('renders polymorphic tags including section, nav, ul, and form with onSubmit', () => {
    const onSubmit = vi.fn((event: Event) => {
      event.preventDefault();
    });
    render(
      <>
        <Stack as="section" testId="sec">section</Stack>
        <Stack as="nav" testId="nav" aria-label="Nav">nav</Stack>
        <Stack as="ul" testId="list"><li>row</li></Stack>
        <Stack as="form" testId="form" onSubmit={onSubmit}>
          <button type="submit">Send</button>
        </Stack>
      </>,
    );
    expect(screen.getByTestId('sec').tagName).toBe('SECTION');
    expect(screen.getByTestId('nav').tagName).toBe('NAV');
    const ul = screen.getByTestId('list');
    expect(ul.tagName).toBe('UL');
    expect(ul.className).toContain('list-none');

    const form = screen.getByTestId('form');
    expect(form.tagName).toBe('FORM');
    fireEvent.submit(form);
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });
});

describe('Inline', () => {
  it('renders a horizontal flex container with default gap-3 and items-center', () => {
    render(
      <Inline testId="inline">
        <span>a</span>
        <span>b</span>
      </Inline>,
    );
    const el = screen.getByTestId('inline');
    expect(el.tagName).toBe('DIV');
    expect(el.className).toContain('flex');
    expect(el.className).toContain('flex-row');
    expect(el.className).toContain('gap-3');
    expect(el.className).toContain('items-center');
    expect(el.className).not.toContain('flex-wrap');
  });

  it('supports gap, align, justify, wrap, id, and custom class', () => {
    render(
      <Inline
        id="row"
        testId="inline"
        gap="2"
        align="baseline"
        justify="end"
        wrap
        class="w-full"
      >
        <span>a</span>
      </Inline>,
    );
    const el = screen.getByTestId('inline');
    expect(el.id).toBe('row');
    expect(el.className).toContain('gap-2');
    expect(el.className).toContain('items-baseline');
    expect(el.className).toContain('justify-end');
    expect(el.className).toContain('flex-wrap');
    expect(el.className).toContain('w-full');
  });

  it('applies flex-nowrap when wrap is explicitly false', () => {
    render(<Inline testId="inline" wrap={false}>nowrap</Inline>);
    expect(screen.getByTestId('inline').className).toContain('flex-nowrap');
  });

  it('carries a group role and its name, so a segmented row is a named radiogroup', () => {
    render(
      <Inline role="radiogroup" aria-label="What this message does">
        <button type="button" role="radio" aria-checked="true">
          Steer
        </button>
      </Inline>,
    );
    const group = screen.getByRole('radiogroup', { name: 'What this message does' });
    expect(group.tagName).toBe('DIV');
    expect(screen.getByRole('radio', { name: 'Steer' }).getAttribute('aria-checked')).toBe('true');
  });
});

describe('Card', () => {
  it('renders with default surface, padding, radius, and border', () => {
    render(<Card testId="card">Body</Card>);
    const el = screen.getByTestId('card');
    expect(el.tagName).toBe('DIV');
    expect(el.className).toContain('bg-surface');
    expect(el.className).toContain('p-6');
    expect(el.className).toContain('rounded');
    expect(el.className).toContain('border');
    expect(el.className).toContain('border-line');
  });

  it('supports all surface tones, paddings, radii, borders, and polymorphic tags', () => {
    const tones: SurfaceTone[] = ['bg', 'surface', 'raised', 'inset', 'sunken', 'rail'];
    const paddings: SpaceStep[] = ['0', '4', '10'];
    render(
      <>
        {tones.map((tone) => (
          <Card key={tone} surface={tone} testId={`tone-${tone}`}>
            {tone}
          </Card>
        ))}
        {paddings.map((pad) => (
          <Card key={pad} padding={pad} testId={`pad-${pad}`}>
            {pad}
          </Card>
        ))}
        <Card
          as="article"
          radius="lg"
          border="strong"
          testId="article-card"
        >
          article
        </Card>
        <Card as="section" radius="sm" border="soft" testId="soft-card">
          soft
        </Card>
        <Card radius="md" border="none" testId="borderless-card">
          none
        </Card>
      </>,
    );

    for (const tone of tones) {
      expect(screen.getByTestId(`tone-${tone}`).className).toContain(`bg-${tone}`);
    }
    for (const pad of paddings) {
      expect(screen.getByTestId(`pad-${pad}`).className).toContain(`p-${pad}`);
    }

    const article = screen.getByTestId('article-card');
    expect(article.tagName).toBe('ARTICLE');
    expect(article.className).toContain('rounded-lg');
    expect(article.className).toContain('border-line-strong');

    const soft = screen.getByTestId('soft-card');
    expect(soft.tagName).toBe('SECTION');
    expect(soft.className).toContain('rounded-sm');
    expect(soft.className).toContain('border-line-soft');

    const borderless = screen.getByTestId('borderless-card');
    expect(borderless.className).toContain('rounded-md');
    expect(borderless.className).toContain('border-0');
  });
});

describe('Text', () => {
  it('renders a span with text-body and text-text by default', () => {
    render(<Text testId="txt">Hello</Text>);
    const el = screen.getByTestId('txt');
    expect(el.tagName).toBe('SPAN');
    expect(el.className).toContain('text-body');
    expect(el.className).toContain('text-text');
    expect(el.textContent).toBe('Hello');
  });

  it('supports all typography sizes and tones without ever emitting text-faint', () => {
    const sizes: TextSize[] = [
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
    const tones: Array<[TextTone, string]> = [
      ['default', 'text-text'],
      ['secondary', 'text-text-secondary'],
      ['muted', 'text-muted'],
      ['accent', 'text-accent-text'],
      ['success', 'text-success'],
      ['warn', 'text-warn'],
      ['danger', 'text-danger'],
      ['info', 'text-info'],
    ];

    render(
      <>
        {sizes.map((size) => (
          <Text key={size} size={size} testId={`size-${size}`}>
            {size}
          </Text>
        ))}
        {tones.map(([tone]) => (
          <Text key={tone} tone={tone} testId={`tone-${tone}`}>
            {tone}
          </Text>
        ))}
      </>,
    );

    for (const size of sizes) {
      expect(screen.getByTestId(`size-${size}`).className).toContain(`text-${size}`);
    }
    for (const [tone, expectedClass] of tones) {
      const cls = screen.getByTestId(`tone-${tone}`).className;
      expect(cls).toContain(expectedClass);
      expect(cls).not.toContain('text-faint');
    }
  });

  it('supports polymorphic elements, htmlFor on label, weight, mono, and truncate', () => {
    render(
      <>
        <Text as="h2" size="title" weight="bold" testId="h2">
          Heading
        </Text>
        <Text as="label" htmlFor="field-1" size="label" weight="medium" testId="lbl">
          Field Label
        </Text>
        <input id="field-1" type="text" />
        <Text as="code" mono truncate testId="code">
          src/ui/Layout.tsx
        </Text>
        <Text as="p" mono={false} weight="semibold" testId="para">
          Paragraph
        </Text>
      </>,
    );

    const h2 = screen.getByTestId('h2');
    expect(h2.tagName).toBe('H2');
    expect(h2.className).toContain('text-title');
    expect(h2.className).toContain('font-bold');

    const label = screen.getByTestId('lbl');
    expect(label.tagName).toBe('LABEL');
    expect(label.getAttribute('for')).toBe('field-1');
    expect(label.className).toContain('font-medium');
    expect(screen.getByLabelText('Field Label')).toBeTruthy();

    const code = screen.getByTestId('code');
    expect(code.tagName).toBe('CODE');
    expect(code.className).toContain('font-mono');
    expect(code.className).toContain('truncate');

    const para = screen.getByTestId('para');
    expect(para.tagName).toBe('P');
    expect(para.className).toContain('font-ui');
    expect(para.className).toContain('font-semibold');
  });

  it('carries its own role, so a hint line can be the live status region of a form', () => {
    render(
      <Text as="p" size="tiny" tone="muted" role="status" testId="hint">
        The name may not contain spaces.
      </Text>,
    );

    const hint = screen.getByRole('status');
    expect(hint).toBe(screen.getByTestId('hint'));
    expect(hint.textContent).toBe('The name may not contain spaces.');
  });
});

describe('SectionHeader', () => {
  it('renders title as h3 with optional eyebrow, description, and actions', () => {
    render(
      <SectionHeader
        testId="sec-head"
        eyebrow="Security"
        title="API Keys"
        description="Manage bearer tokens for automation."
        actions={<button type="button">Create key</button>}
      />,
    );

    const root = screen.getByTestId('sec-head');
    expect(root.className).toContain('justify-between');
    expect(screen.getByText('Security').className).toContain('uppercase');
    const heading = screen.getByRole('heading', { level: 3, name: 'API Keys' });
    expect(heading).toBeTruthy();
    expect(screen.getByText('Manage bearer tokens for automation.')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Create key' })).toBeTruthy();
  });

  it('omits optional eyebrow, description, and actions when not provided', () => {
    render(<SectionHeader testId="minimal-head" title="General" />);
    const root = screen.getByTestId('minimal-head');
    expect(screen.getByRole('heading', { level: 3, name: 'General' })).toBeTruthy();
    expect(root.children.length).toBe(1);
  });
});

describe('SettingRow', () => {
  it('renders title, description, and right-aligned control', () => {
    render(
      <SettingRow
        testId="row"
        title="Compact mode"
        description="Reduce vertical padding in transcripts."
        control={<input type="checkbox" aria-label="Compact mode toggle" />}
      />,
    );

    const row = screen.getByTestId('row');
    expect(row.className).toContain('justify-between');
    const copyCol = row.firstElementChild;
    const ctrlCol = row.lastElementChild;
    expect(copyCol?.className).toContain('min-w-0');
    expect(copyCol?.className).toContain('flex-1');
    expect(ctrlCol?.className).toContain('shrink-0');
    expect(screen.getByText('Compact mode')).toBeTruthy();
    expect(screen.getByText('Reduce vertical padding in transcripts.')).toBeTruthy();
    expect(screen.getByRole('checkbox', { name: 'Compact mode toggle' })).toBeTruthy();
  });

  it('binds a label via htmlFor and displays an error alert when provided', () => {
    render(
      <SettingRow
        testId="row-err"
        title="Session lifetime"
        htmlFor="session-days"
        control={<input id="session-days" type="number" />}
        error="Must be at least 1 day."
      />,
    );

    const input = screen.getByLabelText('Session lifetime');
    expect(input.id).toBe('session-days');
    const alert = screen.getByRole('alert');
    expect(alert.textContent).toBe('Must be at least 1 day.');
    expect(alert.className).toContain('text-danger');
  });
});
