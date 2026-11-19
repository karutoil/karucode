// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { cn, variants, type VariantProps } from './variants';

describe('cn', () => {
  it('joins truthy class strings and ignores falsy or blank entries', () => {
    expect(cn('px-4', false, null, undefined, '', '  ', 'py-2  ')).toBe('px-4 py-2');
  });
});

describe('variants', () => {
  const button = variants({
    base: 'inline-flex items-center rounded-sm',
    variants: {
      tone: {
        default: 'bg-raised text-text',
        primary: 'bg-accent-fill text-on-accent',
        danger: 'bg-danger text-bg',
      },
      size: {
        sm: 'h-[26px] px-4',
        md: 'h-[30px] px-5',
      },
    },
    defaultVariants: {
      tone: 'default',
      size: 'md',
    },
    compoundVariants: [
      {
        variants: { tone: 'primary', size: 'sm' },
        class: 'font-semibold',
      },
    ],
  });

  it('applies base and defaultVariants when called with no props', () => {
    expect(button()).toBe('inline-flex items-center rounded-sm bg-raised text-text h-[30px] px-5');
  });

  it('overrides defaults and applies compoundVariants when matched', () => {
    type Props = VariantProps<typeof button>;
    const p: Props = { tone: 'primary', size: 'sm' };
    expect(button({ ...p, class: 'ml-2' })).toBe(
      'inline-flex items-center rounded-sm bg-accent-fill text-on-accent h-[26px] px-4 font-semibold ml-2',
    );
  });
});
