/* The one place that answers "which part of this label did the search match?".
 *
 * The palette filters rows with a case-insensitive substring test
 * (`${group} ${label} ${hint}`.includes(needle)), and the row then paints the
 * matched run in the accent so the reader can see why it survived. Both halves
 * have to agree: a highlight that marks a different substring than the filter
 * matched is worse than no highlight at all, because it explains the wrong thing.
 * So the split is a pure function over the same lowercase `includes`, and it is
 * tested rather than assumed.
 */

export interface MatchSegment {
  text: string;
  /** True for a run the query matched; those are painted in the accent. */
  hit: boolean;
}

/**
 * Split `text` into alternating plain and matched runs.
 *
 * A blank query, an absent match, or an empty string all come back as one plain
 * segment, so a caller can render the result without a special case. Matching is
 * case-insensitive and repeats for every occurrence, in order — the query
 * `a` on `Aardvark` marks the first letter and both later ones.
 */
export function matchSegments(text: string, query: string): MatchSegment[] {
  const needle = query.trim().toLowerCase();
  if (needle === '' || text === '') return [{ text, hit: false }];

  const haystack = text.toLowerCase();
  const segments: MatchSegment[] = [];
  let from = 0;
  for (;;) {
    const at = haystack.indexOf(needle, from);
    if (at === -1) break;
    if (at > from) segments.push({ text: text.slice(from, at), hit: false });
    segments.push({ text: text.slice(at, at + needle.length), hit: true });
    from = at + needle.length;
  }
  // `from === 0` means nothing matched at all; `from < text.length` means there is
  // a tail after the last hit. Both are plain text.
  if (from === 0) return [{ text, hit: false }];
  if (from < text.length) segments.push({ text: text.slice(from), hit: false });
  return segments;
}
