import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/preact';
import { Field } from './Field';
import { Input } from './Input';

describe('Field', () => {
  it('wires useId() for label htmlFor and render-prop a11y props', () => {
    render(
      <Field label="API endpoint" testId="endpoint-field">
        {({ id, describedBy, invalid }) => (
          <Input
            id={id}
            aria-describedby={describedBy}
            invalid={invalid}
            placeholder="https://example.com"
          />
        )}
      </Field>,
    );

    const input = screen.getByLabelText('API endpoint') as HTMLInputElement;
    const field = screen.getByTestId('endpoint-field');
    expect(input.id).toBeTruthy();
    expect(input.hasAttribute('aria-describedby')).toBe(false);
    expect(input.hasAttribute('aria-invalid')).toBe(false);
    expect(field.hasAttribute('data-invalid')).toBe(false);
  });

  it('wires description and error alert IDs into aria-describedby and invalid state', () => {
    render(
      <Field
        label="Timeout"
        htmlFor="timeout-input"
        description="Seconds before aborting."
        error="Must be at least 10 seconds."
        required
        testId="timeout-field"
      >
        {({ id, describedBy, invalid }) => (
          <Input id={id} aria-describedby={describedBy} invalid={invalid} />
        )}
      </Field>,
    );

    const field = screen.getByTestId('timeout-field');
    const input = screen.getByLabelText(/Timeout/) as HTMLInputElement;
    const alert = screen.getByRole('alert');

    expect(input.id).toBe('timeout-input');
    expect(input.getAttribute('aria-invalid')).toBe('true');
    expect(field.getAttribute('data-invalid')).toBe('true');
    expect(alert.textContent).toBe('Must be at least 10 seconds.');
    expect(alert.id).toBe('timeout-input-error');

    const descEl = document.getElementById('timeout-input-description');
    expect(descEl?.textContent).toBe('Seconds before aborting.');
    expect(input.getAttribute('aria-describedby')).toBe(
      'timeout-input-description timeout-input-error',
    );
    expect(field.textContent).toContain('*');
  });

  it('renders standard non-function children and ignores empty error string', () => {
    render(
      <Field
        label="Workspace"
        htmlFor="ws-input"
        description="Root path."
        error=""
        testId="ws-field"
      >
        <Input id="ws-input" />
      </Field>,
    );

    const field = screen.getByTestId('ws-field');
    const input = screen.getByLabelText('Workspace') as HTMLInputElement;
    expect(input.id).toBe('ws-input');
    expect(screen.queryByRole('alert')).toBeNull();
    expect(field.hasAttribute('data-invalid')).toBe(false);
    expect(screen.getByText('Root path.').id).toBe('ws-input-description');
  });

  it('supports vertical (default) and row layouts with custom class', () => {
    const { rerender } = render(
      <Field label="Compact mode" description="Tighter spacing" testId="layout-field" class="mt-2">
        <Input testId="inner" />
      </Field>,
    );

    let field = screen.getByTestId('layout-field');
    expect(field.className).toContain('flex-col');
    expect(field.className).toContain('mt-2');

    rerender(
      <Field
        label="Compact mode"
        description="Tighter spacing"
        error="Required"
        layout="row"
        testId="layout-field"
      >
        <Input testId="inner" />
      </Field>,
    );

    field = screen.getByTestId('layout-field');
    expect(field.className).toContain('grid');
    expect(field.className).toContain('grid-cols-[minmax(0,1fr)_auto]');
    expect(screen.getByRole('alert').className).toContain('col-span-2');
  });
});
