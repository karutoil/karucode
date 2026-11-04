import { describe, expect, it, vi } from 'vitest';
import { render } from '@testing-library/preact';
import { Markdown } from './Markdown';

// A card that throws on render, which is the case an artifact must never take the
// whole conversation down with.
vi.mock('./ArtifactCard', () => ({
  ArtifactCard: () => {
    throw new Error('artifact exploded');
  },
}));

const TEXT = [
  'Here is the mock you asked for.',
  '',
  '```artifact html title="Signup form"',
  '<!doctype html><html><body><h1>Buy</h1></body></html>',
  '```',
  '',
  'And the prose after it is still here.',
].join('\n');

describe('an artifact that throws while rendering', () => {
  it('takes down its own card and nothing else', () => {
    // The boundary logs through console.error on purpose; that is not a failure.
    const consoleError = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const { container } = render(<Markdown text={TEXT} />);

    const crash = container.querySelector('.crash-card');
    expect(crash).not.toBeNull();
    expect(crash?.textContent).toContain('this artifact could not be rendered');
    // The prose on both sides of the fence survived, and the slot is still there.
    expect(container.textContent).toContain('Here is the mock you asked for.');
    expect(container.textContent).toContain('And the prose after it is still here.');
    expect(container.querySelector('[data-artifact-index]')).not.toBeNull();
    consoleError.mockRestore();
  });
});
