/**
 * Zero-dependency typed variant composer (`cva`-style) for Preact UI primitives.
 *
 * Keeps class composition static-analyzable for Tailwind v4 `@source` scanning
 * and `ui-contract.test.ts`: every variant value is a literal class string from
 * karucode's `@theme inline` token set.
 */

export type ClassValue = string | false | null | undefined;

export function cn(...parts: readonly ClassValue[]): string {
  let out = '';
  for (const part of parts) {
    if (!part) continue;
    const trimmed = part.trim();
    if (!trimmed) continue;
    out = out ? `${out} ${trimmed}` : trimmed;
  }
  return out;
}

type VariantMap = Record<string, Record<string, string>>;

type VariantSelection<V extends VariantMap> = {
  [K in keyof V]?: keyof V[K];
};

export interface CompoundVariant<V extends VariantMap> {
  variants: VariantSelection<V>;
  class: string;
}

export interface VariantsConfig<V extends VariantMap> {
  base?: string;
  variants: V;
  defaultVariants?: VariantSelection<V>;
  compoundVariants?: readonly CompoundVariant<V>[];
}

export type VariantProps<Fn> = Fn extends (
  props?: infer P,
) => string
  ? Omit<NonNullable<P>, 'class'>
  : never;

export function variants<V extends VariantMap>(
  config: VariantsConfig<V>,
): (props?: VariantSelection<V> & { class?: string }) => string {
  const { base, variants: variantMap, defaultVariants, compoundVariants } = config;

  return (props) => {
    const resolved: Record<string, string | number | symbol | undefined> = {};
    const parts: ClassValue[] = [base];

    for (const key of Object.keys(variantMap) as Array<keyof V>) {
      const choice = props?.[key] ?? defaultVariants?.[key];
      resolved[key as string] = choice;
      if (choice === undefined) continue;
      const group = variantMap[key];
      if (!group) continue;
      const cls = group[choice as string];
      if (cls) parts.push(cls);
    }

    if (compoundVariants) {
      for (const entry of compoundVariants) {
        let matches = true;
        for (const key of Object.keys(entry.variants) as Array<keyof V>) {
          if (resolved[key as string] !== entry.variants[key]) {
            matches = false;
            break;
          }
        }
        if (matches) parts.push(entry.class);
      }
    }

    if (props?.class) parts.push(props.class);
    return cn(...parts);
  };
}
