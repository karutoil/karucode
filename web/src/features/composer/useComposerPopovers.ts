import type { RefObject } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';
import { toErrorMessage } from '../../api/client';
import * as api from '../../api/endpoints';
import type { CommandSpec } from '../../api/types';
import {
  activeMention,
  indexFromTree,
  insertMention,
  MENTION_INDEX_DEPTH,
  MENTION_LIMIT,
  rankMentions,
  type ActiveMention,
  type MentionCandidate,
  type TreeLoader,
} from '../../lib/mentions';
import {
  insertCommand,
  knownCommand,
  rankCommands,
  SLASH_MENU_LIMIT,
  slashQuery,
} from '../../lib/slash';
import * as store from '../../state/store';
import { defaultLoadTree, INDEX_TTL_MS } from './helpers';
import type { FieldUpdate } from './types';

export interface ComposerPopoversHandle {
  // Mentions
  mention: ActiveMention | null;
  ranked: MentionCandidate[];
  activeIndex: number;
  menuOpen: boolean;
  indexLoading: boolean;
  indexError: string | null;
  setActiveOption: (update: number | ((current: number) => number)) => void;
  setDismissed: (value: boolean) => void;
  chooseMention: (candidate: MentionCandidate) => void;
  appendUploadedSuggestion: (path: string, size: number) => void;
  // Slash commands
  slashQueryText: string | null;
  slashOpen: boolean;
  projectCatalog: CommandSpec[];
  rankedCommands: CommandSpec[];
  slashActiveIndex: number;
  catalogError: string | null;
  slashCommand: CommandSpec | null;
  setSlashIndex: (update: number | ((current: number) => number)) => void;
  setSlashDismissed: (value: boolean) => void;
  chooseCommand: (command: CommandSpec) => void;
}

export function useComposerPopovers(options: {
  draft: string;
  caret: number;
  projectId: string | null;
  textarea: RefObject<HTMLTextAreaElement | null>;
  setDraft: (update: FieldUpdate<string>) => void;
  setCaret: (update: FieldUpdate<number>) => void;
  loadMentionTree?: TreeLoader;
  loadCommands?: (project: string | null) => Promise<CommandSpec[]>;
}): ComposerPopoversHandle {
  const {
    draft,
    caret,
    projectId,
    textarea,
    setDraft,
    setCaret,
    loadMentionTree,
    loadCommands,
  } = options;

  // ---- @-mentions -------------------------------------------------------
  const [suggestions, setSuggestions] = useState<MentionCandidate[]>([]);
  const [activeOption, setActiveOption] = useState(0);
  const [indexError, setIndexError] = useState<string | null>(null);
  const [indexLoading, setIndexLoading] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const indexedAt = useRef(0);
  const loadTree = loadMentionTree ?? defaultLoadTree;

  // The index is per project. The TTL alone would keep the previous project's
  // file list for up to a minute after a switch, and a mention could then name a
  // path the agent cannot open.
  const indexedProject = useRef(projectId);
  useEffect(() => {
    if (indexedProject.current === projectId) return;
    indexedProject.current = projectId;
    indexedAt.current = 0;
    setSuggestions([]);
  }, [projectId]);

  const mention = activeMention(draft, caret);
  const mentionOpen = mention !== null && !dismissed;
  const ranked =
    mentionOpen && mention ? rankMentions(suggestions, mention.query, MENTION_LIMIT) : [];
  const activeIndex = ranked.length === 0 ? 0 : Math.min(activeOption, ranked.length - 1);
  const menuOpen = mentionOpen && (ranked.length > 0 || indexLoading || indexError !== null);

  useEffect(() => {
    if (!mentionOpen) return;
    const fresh = indexedAt.current > 0 && Date.now() - indexedAt.current < INDEX_TTL_MS;
    if (fresh) {
      setIndexLoading(false);
      return;
    }
    let cancelled = false;
    const requestedProject = projectId;
    setIndexLoading(true);
    setIndexError(null);
    void (async () => {
      try {
        const tree = await loadTree(MENTION_INDEX_DEPTH);
        // A response can outlive both this popover and a project switch. Never
        // let the previous workspace populate the next project's menu.
        if (cancelled || store.currentProjectId.value !== requestedProject) return;
        setSuggestions(indexFromTree(tree).filter((entry) => entry.kind === 'file'));
        indexedAt.current = Date.now();
      } catch (error) {
        if (cancelled || store.currentProjectId.value !== requestedProject) return;
        setIndexError(`Could not index the workspace: ${toErrorMessage(error)}`);
      } finally {
        if (!cancelled && store.currentProjectId.value === requestedProject) {
          setIndexLoading(false);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [mentionOpen, loadTree, projectId]);

  // A new query always starts at the best match.
  useEffect(() => {
    setActiveOption(0);
  }, [mention?.query, mentionOpen]);

  const chooseMention = (candidate: MentionCandidate): void => {
    if (!mention) return;
    const next = insertMention(draft, mention, candidate.path);
    setDraft(next.text);
    setCaret(next.caret);
    setDismissed(false);
    setActiveOption(0);
    // The textarea is controlled: move the caret once the value has rendered.
    window.setTimeout(() => {
      const element = textarea.current;
      if (!element) return;
      element.focus();
      element.setSelectionRange(next.caret, next.caret);
    }, 0);
  };

  const appendUploadedSuggestion = (path: string, size: number): void => {
    setSuggestions((current) =>
      current.some((entry) => entry.path === path)
        ? current
        : [...current, { path, kind: 'file', size }],
    );
  };

  // ---- slash commands ---------------------------------------------------
  //
  // The menu is a *hint*: the server resolves the command from the message, so a
  // stale or failed catalog costs a missing suggestion, never a wrong action. That
  // is why the fetch is lazy (on the first `/`), cached per project, and why a
  // failure still leaves the box usable.
  const catalog = store.commandCatalog.value;
  const catalogProject = store.commandCatalogProject.value;
  const catalogError = store.commandCatalogError.value;
  const catalogTick = store.commandCatalogTick.value;
  const [slashIndex, setSlashIndex] = useState(0);
  const [slashDismissed, setSlashDismissed] = useState(false);
  const loadedCatalogKey = useRef<string | null>(null);
  const loadCatalog = loadCommands ?? ((project: string | null) => api.listCommands(project));

  const slashQueryText = slashQuery(draft, caret);
  const slashOpen = slashQueryText !== null && !slashDismissed;
  // Skills are discovered inside the project's workspace, so another project's
  // list would offer a command the agent cannot read.
  const projectCatalog = catalogProject === projectId ? (catalog ?? []) : [];
  const rankedCommands = slashOpen
    ? rankCommands(projectCatalog, slashQueryText ?? '', SLASH_MENU_LIMIT)
    : [];
  const slashActiveIndex =
    rankedCommands.length === 0 ? 0 : Math.min(slashIndex, rankedCommands.length - 1);

  useEffect(() => {
    if (slashQueryText === null) return;
    const key = `${projectId ?? ''}#${catalogTick}`;
    if (loadedCatalogKey.current === key) return;
    let cancelled = false;
    void (async () => {
      try {
        const list = await loadCatalog(projectId);
        // A response can outlive a project switch; never seed the next project's
        // menu with another workspace's skills.
        if (cancelled || store.currentProjectId.value !== projectId) return;
        store.commandCatalog.value = list;
        store.commandCatalogProject.value = projectId;
        store.commandCatalogError.value = null;
        loadedCatalogKey.current = key;
      } catch (error) {
        if (cancelled) return;
        store.commandCatalogError.value = `Could not load the commands: ${toErrorMessage(error)}`;
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [slashQueryText === null, projectId, catalogTick]);

  // A new query always starts at the best match.
  useEffect(() => {
    setSlashIndex(0);
  }, [slashQueryText]);

  const chooseCommand = (command: CommandSpec): void => {
    const text = insertCommand(command);
    setDraft(text);
    setCaret(text.length);
    // Picking closes the menu: the next Enter is a send, not another pick of the
    // row that is now the whole draft. Typing re-opens it (`onInput` below).
    setSlashDismissed(true);
    window.setTimeout(() => {
      const element = textarea.current;
      if (!element) return;
      element.focus();
      element.setSelectionRange(text.length, text.length);
    }, 0);
  };

  const slashCommand = knownCommand(projectCatalog, draft);

  return {
    mention,
    ranked,
    activeIndex,
    menuOpen,
    indexLoading,
    indexError,
    setActiveOption,
    setDismissed,
    chooseMention,
    appendUploadedSuggestion,
    slashQueryText,
    slashOpen,
    projectCatalog,
    rankedCommands,
    slashActiveIndex,
    catalogError,
    slashCommand,
    setSlashIndex,
    setSlashDismissed,
    chooseCommand,
  };
}
