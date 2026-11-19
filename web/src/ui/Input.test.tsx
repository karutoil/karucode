import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/preact';
import { Checkbox, Input, Select, Textarea } from './Input';

describe('Input', () => {
  it('renders a text input with karucode token utilities and no .input class', () => {
    render(<Input testId="name-input" placeholder="Enter name" />);
    const input = screen.getByTestId('name-input') as HTMLInputElement;

    expect(input.tagName).toBe('INPUT');
    expect(input.type).toBe('text');
    expect(input.placeholder).toBe('Enter name');
    expect(input.className).toContain('bg-rail');
    expect(input.className).toContain('border-line');
    expect(input.className).toContain('h-[32px]');
    expect(input.className).toContain('text-small');
    expect(input.classList.contains('input')).toBe(false);
    expect(input.hasAttribute('aria-invalid')).toBe(false);
    expect(input.hasAttribute('data-invalid')).toBe(false);
  });

  it('supports size variants, mono typography, and custom classes', () => {
    const { rerender } = render(<Input testId="sized" size="sm" mono class="max-w-[200px]" />);
    let input = screen.getByTestId('sized');
    expect(input.className).toContain('h-[28px]');
    expect(input.className).toContain('px-3');
    expect(input.className).toContain('text-tiny');
    expect(input.className).toContain('font-mono');
    expect(input.className).toContain('max-w-[200px]');

    rerender(<Input testId="sized" size="md" mono />);
    input = screen.getByTestId('sized');
    expect(input.className).toContain('h-[32px]');
    expect(input.className).toContain('px-4');
    expect(input.className).toContain('font-mono');
    expect(input.className).toContain('text-message');
  });

  it('wires invalid state and accessibility attributes', () => {
    render(
      <Input
        testId="port-input"
        id="port"
        name="port"
        type="number"
        min={1}
        max={65535}
        step={1}
        value={8787}
        required
        readOnly
        invalid
        aria-label="Port"
        aria-describedby="port-hint"
      />,
    );
    const input = screen.getByTestId('port-input') as HTMLInputElement;
    expect(input.id).toBe('port');
    expect(input.name).toBe('port');
    expect(input.type).toBe('number');
    expect(input.min).toBe('1');
    expect(input.max).toBe('65535');
    expect(input.step).toBe('1');
    expect(input.value).toBe('8787');
    expect(input.required).toBe(true);
    expect(input.readOnly).toBe(true);
    expect(input.getAttribute('aria-label')).toBe('Port');
    expect(input.getAttribute('aria-describedby')).toBe('port-hint');
    expect(input.getAttribute('aria-invalid')).toBe('true');
    expect(input.getAttribute('data-invalid')).toBe('true');
    expect(input.className).toContain('border-danger');
  });

  it('fires onInput, onChange, onKeyDown, onFocus, and onBlur', () => {
    const onInput = vi.fn();
    const onKeyDown = vi.fn();
    const onFocus = vi.fn();
    const onBlur = vi.fn();

    const { rerender } = render(
      <Input
        testId="events-input"
        defaultValue="hello"
        onInput={onInput}
        onKeyDown={onKeyDown}
        onFocus={onFocus}
        onBlur={onBlur}
      />,
    );

    const input = screen.getByTestId('events-input') as HTMLInputElement;
    expect(input.value).toBe('hello');

    fireEvent.focusIn(input);
    fireEvent.input(input, { target: { value: 'world' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    fireEvent.focusOut(input);

    expect(onFocus).toHaveBeenCalledTimes(1);
    expect(onInput).toHaveBeenCalledTimes(1);
    expect(onKeyDown).toHaveBeenCalledTimes(1);
    expect(onBlur).toHaveBeenCalledTimes(1);

    const onChange = vi.fn();
    rerender(<Input testId="events-input" defaultValue="hello" onChange={onChange} />);
    fireEvent.input(screen.getByTestId('events-input'), { target: { value: 'updated' } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('forwards disabled state', () => {
    render(<Input testId="disabled-input" disabled />);
    const input = screen.getByTestId('disabled-input') as HTMLInputElement;
    expect(input.disabled).toBe(true);
  });
});

describe('Textarea', () => {
  it('renders with min-h-[72px], token styling, and no .input class', () => {
    render(<Textarea testId="bio" placeholder="Notes" rows={4} />);
    const textarea = screen.getByTestId('bio') as HTMLTextAreaElement;

    expect(textarea.tagName).toBe('TEXTAREA');
    expect(textarea.placeholder).toBe('Notes');
    expect(textarea.rows).toBe(4);
    expect(textarea.className).toContain('min-h-[72px]');
    expect(textarea.className).toContain('bg-rail');
    expect(textarea.className).toContain('text-small');
    expect(textarea.classList.contains('input')).toBe(false);
  });

  it('supports mono, invalid, disabled, readOnly, and events', () => {
    const onInput = vi.fn();
    const onChange = vi.fn();
    const onKeyDown = vi.fn();

    const { rerender } = render(
      <Textarea
        testId="prompt-box"
        id="prompt"
        name="prompt"
        value="SELECT 1;"
        mono
        invalid
        readOnly
        required
        aria-label="Prompt"
        aria-describedby="prompt-err"
        onInput={onInput}
        onKeyDown={onKeyDown}
      />,
    );

    const textarea = screen.getByTestId('prompt-box') as HTMLTextAreaElement;
    expect(textarea.value).toBe('SELECT 1;');
    expect(textarea.className).toContain('font-mono');
    expect(textarea.className).toContain('text-message');
    expect(textarea.className).toContain('border-danger');
    expect(textarea.getAttribute('aria-invalid')).toBe('true');
    expect(textarea.getAttribute('data-invalid')).toBe('true');
    expect(textarea.getAttribute('aria-describedby')).toBe('prompt-err');
    expect(textarea.readOnly).toBe(true);
    expect(textarea.required).toBe(true);

    fireEvent.input(textarea, { target: { value: 'SELECT 2;' } });
    fireEvent.keyDown(textarea, { key: 'Escape' });
    expect(onInput).toHaveBeenCalledTimes(1);
    expect(onKeyDown).toHaveBeenCalledTimes(1);

    rerender(<Textarea testId="prompt-box" value="SELECT 1;" onChange={onChange} />);
    fireEvent.input(screen.getByTestId('prompt-box'), { target: { value: 'SELECT 3;' } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });
});

describe('Select', () => {
  it('renders options prop and forwards value and onChange', () => {
    const onChange = vi.fn();
    render(
      <Select
        testId="mode-select"
        id="mode"
        name="mode"
        value="read_only"
        aria-label="Approval mode"
        options={[
          { value: 'auto', label: 'Auto' },
          { value: 'read_only', label: 'Read Only' },
          { value: 'locked', label: 'Locked', disabled: true },
        ]}
        onChange={onChange}
      />,
    );

    const select = screen.getByTestId('mode-select') as HTMLSelectElement;
    expect(select.tagName).toBe('SELECT');
    expect(select.value).toBe('read_only');
    expect(select.classList.contains('input')).toBe(false);
    expect(select.options).toHaveLength(3);
    expect(select.options[2]?.disabled).toBe(true);

    fireEvent.change(select, { target: { value: 'auto' } });
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('supports children options and invalid/disabled states', () => {
    render(
      <Select testId="child-select" invalid disabled aria-describedby="sel-err">
        <option value="a">Alpha</option>
        <option value="b">Beta</option>
      </Select>,
    );

    const select = screen.getByTestId('child-select') as HTMLSelectElement;
    expect(select.disabled).toBe(true);
    expect(select.getAttribute('aria-invalid')).toBe('true');
    expect(select.getAttribute('data-invalid')).toBe('true');
    expect(select.getAttribute('aria-describedby')).toBe('sel-err');
    expect(select.className).toContain('border-danger');
    expect(select.options).toHaveLength(2);
  });
});

describe('Checkbox', () => {
  it('renders native checkbox inside label with generated id and description wiring', () => {
    const onChange = vi.fn();
    render(
      <Checkbox
        testId="notify-check"
        checked={false}
        label="Enable notifications"
        description="Notify when a run finishes."
        onChange={onChange}
      />,
    );

    const checkbox = screen.getByTestId('notify-check') as HTMLInputElement;
    const byLabel = screen.getByLabelText(/Enable notifications/) as HTMLInputElement;
    expect(checkbox).toBe(byLabel);
    expect(checkbox.type).toBe('checkbox');
    expect(checkbox.checked).toBe(false);
    expect(checkbox.className).toContain('w-[16px]');
    expect(checkbox.className).toContain('h-[16px]');

    const describedBy = checkbox.getAttribute('aria-describedby');
    expect(describedBy).toBeTruthy();
    const descEl = document.getElementById(describedBy!);
    expect(descEl?.textContent).toBe('Notify when a run finishes.');

    fireEvent.click(checkbox);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  it('toggles from label text click and omits aria-describedby when no description', () => {
    const onChange = vi.fn();
    render(
      <Checkbox
        testId="plain-check"
        id="plain-id"
        checked
        label="Remember choice"
        onChange={onChange}
      />,
    );

    const checkbox = screen.getByTestId('plain-check') as HTMLInputElement;
    expect(checkbox.id).toBe('plain-id');
    expect(checkbox.checked).toBe(true);
    expect(checkbox.hasAttribute('aria-describedby')).toBe(false);

    fireEvent.click(screen.getByText('Remember choice'));
    expect(onChange).toHaveBeenCalledWith(false);
  });

  it('respects disabled state', () => {
    const onChange = vi.fn();
    render(
      <Checkbox
        testId="disabled-check"
        checked={false}
        disabled
        label="Locked option"
        onChange={onChange}
      />,
    );

    const checkbox = screen.getByTestId('disabled-check') as HTMLInputElement;
    expect(checkbox.disabled).toBe(true);
  });
});
