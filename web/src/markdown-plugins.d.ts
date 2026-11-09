/**
 * Type declarations for the two GFM plugins `lib/markdown.ts` adds.
 *
 * Both ship JavaScript only. They are markdown-it plugins - a function that
 * receives the instance and mutates its rules - so the shape is the same for
 * each, and declaring it here is cheaper than a `@types` package that does not
 * exist for one of them.
 */
declare module 'markdown-it-footnote' {
  import type MarkdownIt from 'markdown-it';
  const footnote: MarkdownIt.PluginSimple;
  export default footnote;
}

declare module 'markdown-it-task-lists' {
  import type MarkdownIt from 'markdown-it';

  interface TaskListOptions {
    /** Interactive checkboxes. Off for a rendered document: nothing stores the state. */
    enabled?: boolean;
    /** Wrap the item text in a `<label>` (with `labelAfter`, after the checkbox). */
    label?: boolean;
    labelAfter?: boolean;
  }

  const taskLists: MarkdownIt.PluginWithOptions<TaskListOptions>;
  export default taskLists;
}
