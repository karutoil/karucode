export interface CssDeclaration {
  property: string;
  value: string;
  /** One-based line containing the first character of the property. */
  line: number;
}

export function cssDeclarations(css: string): CssDeclaration[];
