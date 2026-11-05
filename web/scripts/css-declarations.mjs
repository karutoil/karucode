/**
 * Blank comments and quoted strings without moving any remaining character.
 * Declaration-looking prose and semicolons inside either must not become CSS.
 */
function blankIgnored(css) {
  const out = css.split('');
  let quote = '';
  let comment = false;

  for (let index = 0; index < css.length; index += 1) {
    const char = css[index];
    const next = css[index + 1];
    if (comment) {
      if (char === '*' && next === '/') {
        out[index] = ' ';
        out[index + 1] = ' ';
        index += 1;
        comment = false;
      } else if (char !== '\n') {
        out[index] = ' ';
      }
      continue;
    }
    if (quote) {
      if (char === '\\' && next !== undefined) {
        out[index] = ' ';
        if (next !== '\n') out[index + 1] = ' ';
        index += 1;
      } else {
        if (char !== '\n') out[index] = ' ';
        if (char === quote) quote = '';
      }
      continue;
    }
    if (char === '/' && next === '*') {
      out[index] = ' ';
      out[index + 1] = ' ';
      index += 1;
      comment = true;
    } else if (char === '"' || char === "'") {
      out[index] = ' ';
      quote = char;
    }
  }
  return out.join('');
}

/**
 * Return real declarations wherever they start: on their own line, after `{`,
 * or after another declaration. Values may span lines; `line` always identifies
 * the property's original start line. Ignored source is blank in returned values.
 */
export function cssDeclarations(css) {
  const clean = blankIgnored(css);
  const declaration = /(^|\n|[;{])\s*([a-z-]+)\s*:\s*([^;{}]*?)(?=;|\}|$)/g;
  const out = [];
  let line = 1;
  let countedTo = 0;
  let match;

  while ((match = declaration.exec(clean)) !== null) {
    const property = match[2];
    const propertyAt = match.index + match[0].indexOf(property);
    const newlines = clean.slice(countedTo, propertyAt).match(/\n/g);
    if (newlines) line += newlines.length;
    countedTo = propertyAt;
    out.push({ property, value: match[3].trim(), line });
  }
  return out;
}
