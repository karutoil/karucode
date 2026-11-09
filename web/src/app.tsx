// @kc-doc: web-ui
// @kc-title: Use the browser workspace
// @kc-summary: Panels, keyboard shortcuts, plugin-contributed controls and the dev loop for the Preact frontend that ships inside the binary.
// @kc-tags: ui, web, frontend, workbench, panels, context, shortcuts, preact, plugins, buttons
// @kc-applies-to: ui:ProjectRail, ui:project-switcher, ui:mobile-sheet, ui:mobile-layout, ui:mobile-touch-target, ui:mobile-keyboard-inset, env:interactive-widget, route:GET /api/projects/:id/favicon, ui:SessionsRail, ui:RailPeek, ui:ChatView, ui:Composer, ui:TpsBadge, ui:ContextPill, ui:Inspector, ui:ResultResizer, route:PATCH /api/projects/:id/preferences, ui:SessionInfoTab, route:GET /api/sessions/:id/info, file:web/src/components/SessionInfoTab.tsx, file:web/src/lib/sessionInfo.ts, ui:SettingsSheet, ui:NotificationsSection, ui:ToastRegion, ui:NotificationCenter, ui:notification-bell, ui:CommandPalette, ui:FileViewer, ui:FilePreview, ui:RewindDialog, ui:plugin-panel, route:GET /api/workspace/tree, route:PUT /api/workspace/file, route:GET /api/workspace/raw, route:GET /api/workspace/preview/*rest, route:POST /api/workspace/upload, file:web/src/components/Dialog.tsx, route:GET /api/sessions/:id/rewind, route:POST /api/sessions/:id/rewind, event:session_rewound, route:GET /api/plugins/:name/panels/:panel, config:plugin_panels, route:POST /api/plugins/:name/ui/actions/:action, config:plugin_ui, config:ui, file:web/src/components/PluginSlot.tsx, file:web/src/components/uiContributions.ts, file:web/src/components/PluginControls.tsx, file:web/src/state/pluginUiPreferences.ts, file:web/src/components/PluginViews.tsx, file:web/src/components/viewPanelRegister.ts, file:web/src/styles/plugin-views.css, dir:plugins/demos, file:plugins/demos/README.md, file:scripts/verify_plugin_demos.py, cmd:python3 scripts/verify_plugin_demos.py, file:web/src/styles/plugin-slots.css, file:scripts/verify_plugin_ui.py, cmd:python3 scripts/verify_plugin_ui.py, fn:matchUiKeybinding, fn:applyUiPreferences, ui:keybinding, ui:plugin-controls, fn:registerPluginPanelRenderer, ui:PluginSlot, ui:plugin-slot-button, config:[notifications], event:usage, event:notification, route:GET /api/sessions/:id/queues, route:DELETE /api/sessions/:id/queues/:queued_id, event:queued_messages, event:steer_applied, ui:AgentsTab, route:GET /api/agents, route:POST /api/agents/:id/pause, route:POST /api/agents/:id/resume, route:POST /api/agents/:id/cancel, event:agent_spawned, event:agent_paused, event:agent_note, event:agent_message, event:agent_finished, tool:spawn_agent, tool:report_status, file:web/src/components/AgentsTab.tsx, file:web/src/state/agentConsole.ts, file:web/src/components/pluginPanels.ts, file:web/src/state/queues.test.ts, file:web/src/styles/composer.css, file:scripts/verify_queues.py, file:web/src/components/PluginPanelView.tsx, file:web/src/components/RewindDialog.tsx, file:web/src/components/TpsBadge.tsx, file:web/src/components/NotificationsSettings.tsx, file:web/src/styles/mobile.test.ts, file:web/src/state/notifier.ts, file:web/src/state/railPeek.ts, file:web/src/state/notifications.test.ts, file:web/src/components/NotificationCenter.tsx, file:web/src/components/SessionsRail.tsx, file:web/src/lib/notifySound.ts, file:web/src/lib/notifyDesktop.ts, file:web/src/lib/tps.ts, file:web/src/state/promptTokens.ts, file:web/src/lib/dockHeight.ts, file:web/src/lib/viewportInset.ts, file:web/src/lib/menu.tsx, ui:context-menu, fn:registerPluginPanelRenderer, event:tool_call_finished, tool:read_file, config:model.supports_images, cmd:npm run dev, cmd:npm run build, file:web/dist, env:KARUCODE_WEB_DIR, ui:GitManager, ui:GitChanges, ui:GitCommitBox, ui:GitBranchDialog, ui:GitHistory, route:GET /api/git/status, route:GET /api/git/diff, route:GET /api/git/branches, route:POST /api/git/branches, route:POST /api/git/branches/delete, route:POST /api/git/checkout, route:GET /api/git/log, route:GET /api/git/commits/:sha, route:POST /api/git/stage, route:POST /api/git/unstage, route:POST /api/git/discard, route:POST /api/git/commit, route:GET /api/git/stash, route:POST /api/git/stash, file:web/src/components/git/GitManager.tsx, file:web/src/styles/git.css, ui:WorkbenchTaskMenu, ui:ContextDrawer, file:web/src/components/WorkbenchTaskMenu.tsx, file:web/src/components/ContextDrawer.tsx, file:web/src/state/workbench.ts, file:web/src/lib/approvalCopy.ts, file:web/src/styles/workbench.css, file:web/src/styles/workbench-navigation.css, file:scripts/verify_workbench_frontend.py, cmd:python3 scripts/verify_workbench_frontend.py, ui:ControlPlane, route:GET /api/control/overview, route:GET /api/control/sessions, file:web/src/state/controlPlane.ts, file:web/src/components/control-plane/ControlPlane.tsx, file:web/src/components/control-plane/ControlRail.tsx, file:web/src/components/control-plane/ControlOverview.tsx, file:web/src/components/control-plane/ControlSession.tsx, file:web/src/components/control-plane/controlModel.ts, file:web/src/styles/control-shared.css, file:web/src/styles/control-rail.css, file:web/src/styles/control-overview.css, file:web/src/styles/control-session.css, file:web/src/lib/historyLoader.ts, file:web/src/lib/virtualWindow.ts, file:web/src/components/ChatTranscript.tsx, file:web/src/state/chat.ts, file:web/src/lib/streamingMarkdown.ts, file:web/src/components/Markdown.tsx, file:web/src/components/ThinkingBlock.tsx, ui:streaming-render, file:web/src/api/sharedStream.ts, ui:shared-event-stream, route:GET /api/events, file:web/src/components/ToolCard.tsx, file:web/src/components/CommandCard.tsx, file:scripts/verify_tps_badge.py, cmd:python3 scripts/verify_tps_badge.py, file:web/src/state/draftSession.test.ts, file:scripts/verify_draft_session.py, cmd:python3 scripts/verify_draft_session.py
//
// # Layout
//
// * **Workbench ownership**: `App` mounts either the control plane or the ordinary
//   `SessionsRail` + `ChatView`, together with global overlays. In the ordinary
//   workspace `ChatView` owns the session header, workspace views, conversation,
//   Composer, the one `Inspector` results dock and its sibling `ContextDrawer`.
//   There is no second App-level inspector. Full Source control and Terminal are
//   mounted on first use and then hidden rather than remounted as views change.
// * **Project rail** (64 px where shown): projects use their real `favicon.ico`
//   or initials, with **Theme studio**, command-palette and settings shortcuts.
//   App mounts it only for the control plane or for an intentionally collapsed
//   desktop task rail, where it keeps compact project switching and RailPeek.
//   The expanded ordinary Workbench instead puts project switching, management
//   and the control-plane link in `SessionsRail`. On an ordinary narrow layout
//   the sessions sheet carries those project controls (see **Mobile**).
// * **Control plane**: one browser of every project, split into two groups. The
//   **Control chats** group (above) lists conversations that act on *other*
//   projects: each row carries the title (or *Untitled chat*), a live/total
//   subagent badge, its status word and how long ago it last moved. The model a
//   chat is pinned to is deliberately *not* on the row - a name is an identity,
//   and the composer is the one place that says which model will answer. The
//   **Sessions** group below lists the ordinary project
//   conversations — search, status filters, running first, project glyph and
//   name — and a session the server marks `control` is never drawn there as
//   well. **＋ New** creates a chat; a row's kebab renames (`PATCH
//   /api/sessions/:id`) or deletes (`DELETE /api/sessions/:id`, after a
//   confirmation, and deleting the open chat returns to the overview). Selecting
//   a project session switches to its project and opens its ordinary chat; a
//   control chat is opened where it is, in its own workspace with no project, so
//   its `@` menu lists projects instead of files. With nothing selected the pane
//   is a dashboard drawn from **one** read (`GET /api/control/overview` via
//   `state/controlPlane.ts`): counters for running sessions, chats, sessions, live
//   agents and projects, running work as cards, a recent-sessions table, and
//   columns for what needs attention, the live agents (with a link to the session
//   they belong to) and the projects with their own session/running counts. A
//   session or chat parked on an unanswered approval is called out above the
//   dashboard and listed under **Waiting on you**, because a run that cannot
//   proceed without a person is not "running" in any useful sense. A failed read
//   never empties it: the last snapshot stays and is labelled
//   `stale · <reason>`. The launch box names a project and takes a draft, and it
//   also picks the model and reasoning effort of the chat it will open — the chat
//   is pinned (`PATCH /api/sessions/:id/model`) *before* the draft is handed to
//   the composer, so the first message already runs on the model that was shown.
//   From 1560px the dashboard is two columns; the whole block is capped at 1880px
//   so a 4K monitor does not strand a narrow column of text. An open conversation
//   is the ordinary `Composer`, so its **model** and **reasoning** selectors live
//   where they always do — in the composer, and they stay mounted for as long as a
//   session is open (the pin is held outside the session lists, so an open chat
//   whose row has not arrived yet keeps its controls). The session header does not
//   repeat them. An inspector column holds the chat's facts (where, state,
//   messages, session id, and *Open in project* / *Stop this run*) plus a
//   **Subagents** list read per session from `GET /api/agents?session=`, so a
//   finished child is still listed — name, role, status word, what the agent says
//   it is doing, its iterations and a link into its own transcript — with a quiet
//   empty state when the chat has delegated nothing. The inspector is a column
//   from 1400px and stacks under the transcript below that; the rail hides behind
//   the back button on a phone, and opening the plane moves focus into its rail.
//   The list refreshes every five seconds; leaving the plane (a project tile in
//   the k. rail, or the sessions rail's project switcher) re-reads the open
//   project's session list, the same read a project switch makes, so the
//   workbench rail never inherits a row the plane was showing for another project
//   or a control chat.
//   See `central-control` for the tools, dispatch and approval semantics. Its
//   embedded conversation is a real `ChatView` with `mode="control-plane"`: that
//   mode does not mount the project-scoped Inspector or Context drawer. Composer
//   still shows reported Usage telemetry, but without a Usage callback it is
//   read-only, not a link to a project drawer.
// * **Sessions rail**: the current project's disclosure and management actions,
//   session search (`/`), available status filters and sessions grouped by last
//   activity with running/failure indicators. **New session** (`Ctrl/Cmd+N`, the
//   rail button, the palette row, a starter prompt) starts a **draft** — see
//   **Starting a conversation** below; nothing is written until the first
//   message, so an abandoned press leaves no empty session behind. On a phone it
//   is a full-height sheet, headed by the project
//   switcher and including **Control plane** and **Settings**. The rail holds the
//   open project's sessions and nothing else: a control chat is never listed
//   there, and opening a conversation that belongs to another project (a
//   notification, the palette) moves the workspace onto *that* project first, so
//   the rail, the file tree and the terminals all describe the session's own
//   project.
// * **Peeking at the collapsed rail**: `Ctrl/Cmd+B` gives the sessions rail zero
//   width and mounts the compact project rail. Hovering a project tile shows *that
//   project's* sessions as a fly-out beside it — same rows, same rename/delete
//   menus, no need to
//   restore the rail to open a session. It is `position: fixed` against the project
//   rail so the conversation never resizes to make room, it closes when the pointer
//   leaves (after a ~260 ms grace period) or on Escape, and it never appears while
//   the rail is open, on a phone, in the control plane, or behind an open dialog.
//   Hovering the project you are already in reads nothing (the store has that list);
//   hovering another costs one bounded read of its sessions, cached for 15 s by
//   `src/state/railPeek.ts`, because the store only holds one project's sessions at
//   a time. Opening a row from another project switches project first, and the
//   fly-out offers **Open this project** for the same reason.
// * **Unread counts on the rail**: each project tile carries an iOS-style counter of
//   its unread notifications, so a run that finished in a project you are not
//   looking at is visible without opening anything. The number is in the tile's
//   accessible name and tooltip as well as in the pill, it is capped at `99+`, and
//   it clears when the notifications behind it are read.
// * **Conversation and progressive disclosure**: streaming Markdown, the model /
//   reasoning selectors, @-file mentions, attachments, diff review and approval
//   controls remain real. Built-in themes start with compact tool/command **rows**
//   and **completed-turn grouping** whose working steps are **open**: the model's
//   narration, the folded runs of calls and the calls inside them are on screen,
//   with **Hide the working** as the header that folds them away again. Each turn
//   and each run of calls keeps its own state, so folding one row never collapses
//   the rest. A row opens its actual tool output or separate command stdout/stderr.
//   Expanded command output owns one bounded scrolling region; its wrapper does
//   not add a second nested scrollbar.
//   Tool failures open to their explanation;
//   running work, pending approvals, errors, interruptions and notices remain
//   discoverable rather than disappearing into a fold. Full activity remains an
//   explicit **Grouping → None** option; **Cards** remains the alternate presentation
//   in Settings → General → Transcript or Theme Studio, which also offers
//   **Working steps → Folded** for the terse view where a finished turn sits behind
//   **Show the working**.
//   Returned images stay visible above the output disclosure and open in the real
//   lightbox; their bytes come from `tool_call_finished.images` live and stored
//   `messages.images` after reload. A model's reasoning is stored on the same
//   assistant row (`reasoning_content`), so the **Thought** block a turn streamed
//   is still there when the session is re-opened, switched to, or the page is
//   reloaded - including a turn that only thought and then called a tool, which
//   has no prose of its own.
// * **Long transcripts**: opening a session, scrolling a history of thousands of
//   rows and keeping up with a live run are all bounded. The transcript is
//   **windowed**: only the rows around the viewport are mounted, with padding
//   standing in for the rest, so a fully loaded conversation does not mount
//   thousands of Markdown blocks at once (see `web/src/lib/virtualWindow.ts`).
//   Older history is not a button: when the reader reaches the line above the
//   oldest loaded row, the next page is fetched automatically and the scroll
//   position is moved by exactly the height the page added, so the row being
//   read stays put (`web/src/lib/historyLoader.ts`; the Agents tab uses the same
//   loader for one agent's activity). A silent refresh - a reconnect, a stream
//   resync, a run settling - keeps the pages already loaded and merges the
//   server's newer tail into them instead of throwing them away; a forced reload
//   after a rewind replaces them. Only the session's own rows are paged: a
//   subagent's transcript belongs to `GET /api/agents/:id`, so a long subagent
//   cannot consume the parent's page.
// * **A live answer stays cheap to draw**: while a message is still arriving it
//   is rendered in two parts - the blocks that have finished, as Markdown parsed
//   once, and the block still being written, re-parsed per delta
//   (`web/src/lib/streamingMarkdown.ts`). Re-parsing the whole message on every
//   token made a long answer cost more per token than the one before it: measured
//   in the shipped bundle, 120 KiB of answer was ~50 ms per render and the tab
//   dropped to 14 fps, which is what "the frontend starts to lag while a run is
//   going" actually was. The split keeps a token's cost at the size of the block
//   being written; the finished message renders once, fully, when it settles, so
//   nothing about the settled transcript changes. Three things are deliberately
//   coarser while live, because each is re-done per token: a fence that has not
//   closed is drawn unhighlighted, a tail longer than 4 096 characters is drawn as
//   text, and artifact cards wait for the fence to close. The reasoning block
//   follows the same rule: a live thought shows its newest 6 000 characters
//   instead of re-laying out the whole thing per delta, and it follows the newest
//   line on an animation frame rather than per token - and stops following once
//   the reader has scrolled up to read an earlier line.
// * **Rewind**: every persisted user message and every assistant turn carries a
//   **Rewind** button in its header (hover or keyboard focus reveals it). It
//   opens a dialog that hides the conversation from that message onwards and
//   offers three choices: **Just remove it** (the conversation ends there; edit
//   the prompt yourself), **Ask again** (the same request goes back to the
//   agent) and **Send a note instead** (a different instruction replaces it).
//   A separate *Revert* checkbox is **off** by default and lists every file the
//   agent changed after that message, with **Restore** or **Remove** beside each
//   one - and an explicit warning when a file has no saved before-image or when
//   the turn ran a tool (`run_command`, a plugin, MCP) whose effects cannot be
//   undone. The list comes from `GET /api/sessions/:id/rewind`, the same preview
//   the server replays, so the dialog describes what will happen rather than an
//   estimate of it. If that preview cannot be loaded, Rewind stays disabled and
//   says the file consequences are unknown rather than claiming none changed.
//   The button is hidden while a run is in flight (the server
//   refuses a rewind of a live session) and for items with no stored row
//   (optimistic sends, live placeholders). The `http-api` topic documents the
//   request shapes.
// * **Composer dock**: the composer *floats* over the bottom of the transcript
//   instead of taking a band of its own, so the conversation keeps the full height
//   of the pane. Its input surface and status stay opaque while reading history:
//   `data-reading-history` never turns the draft into a translucent overlay. The
//   input grows with the draft to its responsive cap, and the transcript reserves
//   the dock's measured height (`--composer-h`), so the last message and **Jump to
//   latest** stay clear of the controls. The gutter either side is not a hit target,
//   so a wheel or drag there still scrolls the transcript. On a running phone the
//   model controls and Stop keep one row while Usage and the explicit queue/steer
//   controls keep another; neither Stop nor the pending-work controls disappear.
// * **Auto-follow**: while you are at the newest message the transcript keeps up
//   with everything that lands - streamed text, Markdown that renders late, tool
//   cards, command output, approval cards - however much arrives at once. Scroll
//   up and it stops following immediately and offers **Jump to latest**; sending
//   a message re-pins it. The rule it uses (`web/src/lib/stickyBottom.ts`) is not
//   "am I within 48px of the bottom": a browser frame dispatches the scroll event
//   from a follow *before* the next one can measure it, so with fast tools a
//   single frame can add more than the threshold and the view would read its own
//   backlog as you scrolling away. Instead it records where following left the
//   viewport and treats a gap no wider than the content that has arrived since
//   then as its own to close. Any movement of the scroll position is yours, and
//   is never overridden.
// * **Workspace views**: *Conversation*, full-width *Source control* and
//   *Terminal* when the server enables terminals, all inside the main task header
//   with their real changed-file/terminal counts, plus **one closable tab per open
//   file** (see *File tabs* below). The standalone workspace strip is
//   gone; when the container is constrained, the same tablist wraps to a second row
//   inside that header without dropping branch, notification, Context, command or
//   task actions. ChatView mounts Source control and Terminal on demand, then keeps
//   an established view mounted while it is hidden, so switching views does not
//   discard old work. Composer belongs to the Conversation view. Tab keys wrap with
//   **Left/Right**, jump with **Home/End**, and scroll the selected tab into view;
//   theme/font changes update the existing terminal without losing its buffer.
// * **Task actions menu**: the session header's kebab owns **Copy conversation**,
//   **Session context**, **Commands**, **Theme studio**, **Settings** and **All
//   tasks**, wired to the existing transcript copier and application actions rather
//   than placeholders. Control-plane chat omits the project Context and All tasks
//   entries. ArrowUp/ArrowDown and Home/End move through enabled items; Escape and
//   Tab dismiss the menu, with Tab restoring the trigger rather than leaking focus
//   into a disappeared menu.
// * **Menus and right-click** (`lib/menu.tsx`, DESIGN-SYSTEM §5B): every popup menu
//   in the product is the same box on the same layer with the same keyboard
//   contract, implemented once. Opening focuses the first *enabled* item, the
//   arrows and Home/End walk enabled items with exactly one item in the Tab order,
//   and Escape or Tab closes and returns focus to the trigger. A menu opened by a
//   **right-click** is portalled into `<body>` on the `.menu-layer` (z-index 90 —
//   above the app chrome, below the dialogs it can be opened from inside), measured
//   after mount and clamped to the viewport, and it closes on scroll or resize
//   because its anchor is a snapshot of where the pointer was. Rows that answer a
//   right-click offer the actions they already offer by a visible control: session
//   rows (both rails and the control plane's dashboard) offer Rename, Copy title and
//   Delete, git history rows offer Copy sha and Copy subject, a changed-file row
//   offers Copy path, file-tree rows offer Open and Copy path, notification rows
//   offer Open, Mark as read and Copy text, agent cards offer Details, Pause/Resume,
//   Stop and Copy agent id, and terminal chips offer Rename and Close. A right-click
//   is never the only path to an action — touch and the phone sheet have no
//   right-click — and it is never taken from a text field, an editable region, an
//   open menu, or a surface where the browser's own menu is the better one (the
//   transcript, the file viewer, a diff, the terminal). Where a row's text is
//   selectable the menu keeps a Copy item, so suppressing the native menu never
//   costs the ability to copy it.
// * **Results dock** (`Inspector`, owned by ChatView): **Files**, **Git**, **Session
//   usage**, **Agents** and one tab per enabled **plugin panel**. File rows open the
//   real viewer; Context's result links select these same tabs through
//   `store.inspectorTab`. The selected result and the desktop collapse preference
//   (`karucode.inspectorCollapsed`) survive opening and closing Context; collapse /
//   reveal uses the existing `toggleInspector` action and persisted preference.
//   Collapsing it hands the whole centre pane back to the conversation, which then
//   keeps the app's content measure (the composer's own 820px box); the narrower
//   70ch reading measure belongs to the side-by-side split alone, so hiding the
//   dock never narrows the transcript.
// * **Result resizing**: the wide split has a focusable 24px separator. Drag with a
//   mouse or touch; **Left/Right** resize by 10px, **Shift+Left/Right** by 50px and
//   **Home/End** choose the safe bounds. The preference is a project property saved
//   with `PATCH /api/projects/:id/preferences`, so a fresh browser/device restores
//   it through `GET /api/projects`. Dragging writes on release and keyboard bursts
//   coalesce. A narrower viewport gets a temporary clamp without saving that clamp;
//   failed writes retain the local choice and explain that it was not saved. In a
//   wide split the fixed separator and Results stay flush with the workspace's
//   right edge during the gesture; no width animation can leave a trailing gap.
//   Narrowing Results transfers the entire released width to Conversation instead
//   of leaving dead space beside its separator. On a wide user-resized split, the
//   transcript matches the composer's box rather than filling the Conversation pane:
//   assistant messages and tools stay left-anchored while compact user messages
//   remain on the right.
// * **Context drawer** (`Ctrl/Cmd+J`): a transient sibling overlay driven by
//   `state/workbench.ts`, with **Now**, **Usage** and **Access**. It does not replace
//   or unmount the Inspector, conversation, full Git or terminal below it. **Now**
//   reports the actual project/session/workspaces, connection and activity, labels
//   the next run's model as a session pin or install default, and distinguishes the
//   tool-reachable workspace from prompt input: opening a panel attaches neither its
//   contents nor a UI snapshot. Only the composed message and deliberately added
//   attachments or @ paths become message input. Its Files / Source control /
//   Session usage / Agents / plugin links reveal the real result selection.
// * **Context Usage and Access**: Usage reuses the server-backed `SessionInfoTab`
//   (not transcript arithmetic), while Access renders the Composer's shared
//   `describeApprovals` copy for the install's real mode and allowlist counts.
//   **Open approval settings** closes Context and opens the existing Settings sheet;
//   Access never becomes a second policy editor. A run rail above all three sections
//   keeps activity, pending approvals and errors linked back to the real
//   conversation controls.
// * **Responsive results**: `styles/workbench.css` splits Conversation and Results
//   in wide Workbench containers, turns Results into a bounded overlay at
//   intermediate container widths, and reuses the existing full-height inspector
//   sheet below 1024px. The separator is absent from both non-split modes. Context
//   is likewise a bounded desktop drawer and a trapped
//   full-height phone sheet. Closing phone Context restores the sheet beneath it;
//   selecting a result or Conversation deliberately navigates to that real view.
// * **Source control** (full workspace view and Results → Git): a git manager, not a
//   read-only diff list. Changed files are grouped into **Merge changes**,
//   **Staged**, **Changes** and **Untracked** - a path with both a staged and a
//   working-tree side appears in both groups, which is git's own model - and
//   each row stages, unstages or discards one file, each group does the same for
//   all of its files, and the badge letter is repeated as a word so colour is
//   never the only signal. The commit box takes a message (`Ctrl/Cmd+Enter`
//   commits) with an **Amend** toggle; with nothing staged, **Commit** stages
//   every change first and says so under the box. The branch chip opens a dialog
//   that filters, switches, creates (optionally switching) and deletes local
//   branches. The full workspace view and Results → Git both use the real
//   `GitManager`: the page layout puts the selected file's patch beside the list,
//   with commit history and one commit's files and patch on click. `.result-git`
//   adapts that manager to the result dock's actual container width instead of
//   substituting a read-only diff. Only **local** operations are offered -
//   staging, commits, branches, stashes; fetch/pull/push need credentials and a
//   prompt, so they stay in the Terminal tab. Discarding changes, deleting an
//   unmerged branch and dropping a stash ask for confirmation first: discarding
//   overwrites the working tree and deletes an untracked file, and none of the
//   three can be undone from the UI.
// * **Agents tab** (inspector): every subagent in the project, live first. A
//   running or paused agent has **Pause**/**Resume** and **Stop** buttons that
//   call `POST /api/agents/:id/{pause,resume,cancel}`; a paused one is still
//   live (it holds a subagent slot) and says so in the status chip. Each row
//   shows what the agent last reported about itself (`report_status`) with a
//   progress bar, and a `background` badge when the spawn detached. Its **Open
//   details** button opens the live transcript without overloading the row's
//   independent Pause/Stop controls. The list itself is read from
//   `GET /api/agents?project=` when the tab opens, when it is asked to refresh,
//   and whenever the session list gains, loses or renames a row - the rows are
//   grouped and labelled by session. A run boundary does not re-read it: status
//   and notes arrive on the event stream, and that response is every agent the
//   project ever spawned.
// * **Session tab** (inspector): what this session *is* and what it *cost*,
//   read from `GET /api/sessions/:id/info` rather than folded from the
//   transcript on screen, because the transcript is a bounded window and the
//   token rows are persisted per provider call. It shows the context window fill
//   (the newest main-agent prompt over the model's own `context_window`), input
//   / output / cached tokens with a cache hit rate, an optional cost estimate
//   when the model declares per-million prices, prompts vs turns vs model calls
//   vs tool calls, runs by outcome, working time (sum of finished runs, so idle
//   gaps are excluded), the per-tool breakdown, one row per subagent with its
//   own tokens, cache rate, message count and duration, and the newest runs.
//   It refreshes on every `usage`, `run_finished` and `agent_finished` frame,
//   coalesced into one read. The rule throughout: a figure the server did not
//   report reads as "—" or "Not reported", never as 0 — an unknown prompt-cache
//   rate is not a 0% hit, and a model with no declared window gets no
//   percentage.
// * **File viewer** (modal or workspace tab): Markdown and HTML open on a rendered
//   preview (with a
//   *Preview*/*Source* toggle), images and PDFs render from
//   `GET /api/workspace/raw`, and every complete text file can be edited and
//   saved. An HTML file is previewed as a **real page**, loaded from
//   `GET /api/workspace/preview[‑project]/<path>` in an `<iframe
//   sandbox="allow-scripts">` (never `srcdoc`): that gives it a base URL inside the
//   workspace, so its own `./style.css`, `../img/logo.png` and `./app.js` load, and
//   the server's policy names *this server's own origin* as the only allowed
//   resource source while keeping the document an opaque origin with
//   `connect-src 'none'` - so it still cannot reach the app, its storage or the
//   network. (`'self'` cannot do that job: inside an opaque origin it matches
//   nothing, which is why every relative asset of every preview used to be
//   refused.) The project, when there is one, travels in the **path**
//   (`/api/workspace/preview-project/<project>/<path>`) because a relative asset
//   reference drops the query string. Fonts and ES modules still cannot load: both
//   need `Access-Control-Allow-Origin`, which this route deliberately does not send.
//   The preview shows the *saved* page - an unsaved draft says so and
//   needs a save - and on a `KARUCODE_AUTH_TOKEN` deployment, where an `<iframe>`
//   cannot present a bearer header, it explains that instead of showing a blank
//   frame. Markdown follows your edits live, so *Preview* is a live preview of the
//   draft. Every preview kind fills its host: no `max-width` measure on the
//   scrolling surface, because a `ch` cap there shrinks the pane itself and
//   leaves a blank strip beside the prose. `Ctrl/Cmd+S` saves; a save whose
//   `expected_size` no longer matches the
//   file offers **Overwrite anyway** rather than clobbering the agent's work;
//   closing with unsaved changes asks first. Binary and server-truncated files
//   cannot be edited (a truncated save would discard everything past the read
//   limit) and the *Edit* button's tooltip says which case applies.
// * **File tabs**: the explorer's row menu (**Open in a tab**) and the viewer's
//   own **Open in tab** button put the same document in the workspace strip beside
//   the conversation instead of the modal; **Open in dialog** puts it back. A tab
//   keeps its read, its scroll position and an unsaved draft while you visit
//   another tab or the conversation, and closing it - the strip's ✕, the panel's
//   close button, or **Delete**/**Backspace** on the selected tab - either drops it
//   (selection moves to the tab that took its place) or, with unsaved edits, asks
//   first. Moving a file between the dialog and a tab is refused while it has
//   unsaved edits, since the other host would reload it from disk. A link inside a
//   rendered preview replaces the dialog's document but opens its own tab from a
//   tab, so following a link never overwrites the page being read. File tabs belong
//   to the project: switching projects closes them.
// * **Rendered Markdown** is GitHub's: tables, `~~strikethrough~~`, task lists
//   (`- [ ]`, read-only checkboxes), footnotes with in-page references,
//   fenced code, and bare-URL autolinks that open in a new tab. An image in a reply
//   is fetched through the API when it points into the workspace and shown from a
//   `blob:` URL (one fetch per file, shared by every rendered copy, so a
//   scroll-windowed transcript does not re-download it); an `https:` image becomes
//   a link, because this app's policy allows no remote image, and a reference that
//   escapes the workspace says so instead of rendering a broken glyph. The chat
//   transcript and the file viewer's Markdown preview are the same renderer, so a
//   document reads identically in both.
// * **Dialogs and the image lightbox**: clicking a tool card's image thumbnail
//   opens a lightbox with the full image. Every dialog (`Dialog.tsx`) renders
//   through a portal into `document.body` rather than in place: `.dialog-layer`
//   is `position: fixed`, and a transcript row carries `content-visibility:
//   auto`, which makes the row a containing block for fixed descendants - so an
//   in-place lightbox was laid out *inside its message row*, measuring 691x575 in
//   a 1440x817 viewport instead of covering the screen. Portalling fixes that for
//   every modal and survives any future `filter`/`transform`/`contain` ancestor.
//   Dialogs keep focus inside, close on Escape, lock scroll and restore focus;
//   invisible controls are skipped, and a sheet's search, rename field or menu
//   can handle its own Escape before the sheet closes. Dialogs mount only while
//   open.
// * **Theme studio** (right drawer, `Ctrl/Cmd+Shift+T`, **Task actions → Theme
//   studio**, or the project-rail palette button when that rail is present):
//   built-in themes, per-token colours, density, transcript options and custom
//   CSS, persisted in the browser. The `themes` topic covers the storage key and
//   the shareable export format.
// * **Settings sheet** (modal): five tabs, split by who owns the setting —
//   **General** (appearance: theme, density, colour mode, transcript layout, and
//   a door into the Theme studio), **Models** (provider profiles, their model
//   libraries, plus sampling under *Advanced*), **Agent** (approvals and
//   subagents; subagent budget, timeouts and raw limits under *Advanced*),
//   **Notifications** (server routing, then this browser, with delivery tuning
//   under *Advanced*) and **Plugins & MCP** (lifecycle and diagnostics). Each
//   tab's *Advanced* block is collapsed by default, because those are the knobs
//   whose defaults are right for nearly every install. A rejected field takes you
//   to its tab and opens the disclosure that holds it — the Save button is
//   shared, so the error could otherwise be off-screen. Panels stay mounted
//   (merely `hidden`) so switching tabs never discards what was typed.
// * **Command palette** (`Ctrl/Cmd+K`): sessions, projects, actions.
//
// # Notifications
//
// One `notification` event from the server feeds five surfaces, and
// `src/state/notifier.ts` is the only place that decides which one fires:
//
// * an in-app toast that names the session and offers **Open**;
// * an unread count on the session row, on the project's tile in the rail, and in
//   `document.title` (`(2) Fix the login redirect — karucode`), which is the only
//   thing visible from another tab;
// * a synthesized sound cue (see below);
// * a desktop notification, when the browser allows one and the event is not about
//   the session you are already looking at;
// * an entry in the **notification centre** (the bell in the chat header, and
//   *Open notifications* in the command palette) — a newest-first list of everything
//   that arrived, with **Mark all read**, per-row **Mark as read**, **Clear**, and a
//   click that opens the session behind the row (switching project when it lives in
//   another one).
//
// The notifier is fed from the event dispatch path, **before** the current-session
// filter, because the server reports every terminal run — a run finishing in
// another project is exactly the case this exists for.
//
// The five surfaces are not five counters: `store.notifications` is one list, and the
// session dot, the project counter, the title total and the centre are all derived
// from it (`store.unread`, `store.unreadByProject`, `store.unreadTotal`), so marking
// anything read clears all of them and they can never disagree. The list is persisted
// per browser (`karucode.notifications`, capped at 100 entries and pruned after a
// week), because the server keeps no notification table: they describe live state.
//
// Sounds are synthesized with Web Audio rather than shipped as files: the page CSP
// has no `media-src`, so it falls back to `default-src 'self'` and a `blob:` audio
// URL is refused by the browser. `src/lib/notifySound.ts` builds each cue from two
// or three oscillators with an envelope, which also means a test can render it and
// measure it. It is deliberately independent of `Notification.permission`: a
// browser that refused notifications still plays the cue.
//
// `src/lib/notifyDesktop.ts` owns the permission rules — never ask on load, ask only
// from the settings button, report "insecure context" instead of failing silently,
// and report "limited" where a mobile browser would need a service worker.
//
// # Starting a conversation
//
// **New session** does not create a session. The rail button, `Ctrl/Cmd+N`, the
// palette row, the collapsed rail's fly-out, a starter prompt and the empty
// state's **New session** all open a **draft** — an empty conversation the client
// holds in memory. Nothing is written to the server until there is something to
// keep, so a press that is immediately abandoned cannot leave another empty row in
// the rail (or on disk).
//
// * The first message creates the session (`POST /api/sessions`) and is sent into
//   it; the server titles the untitled session from that message. The model and
//   reasoning effort chosen in the composer before sending ride along in the same
//   create request, so the first turn already runs on the model the pills showed.
// * A file large enough to be **uploaded** also creates it: an upload lives under
//   `.karucode/attachments/<session>/`, so the session must exist first. A small
//   text file that is inlined into the message does not.
// * Opening any other session, or switching project, discards the draft — the
//   conversation, the file tree and the terminals are all project-scoped.
// * A second **New session** press keeps the draft you have and only puts the
//   caret back in the box; it never throws away what is already typed.
// * A create that fails is reported as a toast and the draft stays, text and
//   attachment chips included: sending again is the whole retry.
//
// While a draft is open the header names it **New session**, the starter prompts
// fill the box instead of opening a session, and the composer, model and reasoning
// controls work exactly as they do in a saved conversation.
// `web/src/state/draftSession.test.ts` pins "the button writes nothing" and "the
// first message writes exactly one session"; `python3 scripts/verify_draft_session.py`
// proves both in real Chromium against a real server (including that the model
// picked while drafting is the model the session is created with).
//
// # Composer
//
// The box floats over the transcript as a *dock* instead of occupying its own
// row (see **Composer dock** above). What that means in practice:
//
// * the input and status stay opaque while you read history; scrolling away does
//   not put transcript text behind a translucent control;
// * it starts one line tall, grows with the draft and on resize, then scrolls at
//   the responsive `--compose-input-max` cap;
// * the transcript reserves the dock's measured height
//   (`--composer-h`, published by `lib/dockHeight.ts`), so the last message is
//   never hidden under it.
//
// Attach context three ways, all of which end up in the message the model reads:
//
// * type `@` to search the workspace index (`GET /api/workspace/tree?depth=6`;
//   `node_modules`, `.git`, `target`, `dist` and similar noise directories are
//   filtered out). Up/Down move, Enter or Tab inserts, Escape dismisses; a path
//   with spaces is inserted quoted (`@"docs/my plan.md"`);
// * press **Attach files** (the paperclip) to pick files;
// * drag files onto the composer, or paste them — a dashed overlay marks the drop
//   target.
//
// Files up to 64 KiB that look like text are inlined into the message as a fenced
// block (the head of the file is sniffed for NUL bytes). Everything else (binary,
// larger than 64 KiB) is uploaded with `POST /api/workspace/upload` and referenced
// by its workspace path, `.karucode/attachments/<session>/<name>`, which the agent
// can open with its file tools; names are never overwritten. Files above the 8 MiB
// upload ceiling are refused before any request is made. Sending is blocked while
// a chip is still reading or uploading, and a failed chip says why.
//
// # Steering a run, following up, and recalling
//
// While the agent works, the same box **queues** instead of sending, because the
// API refuses a second run (`409`). Two kinds, chosen by the toggle that appears
// beside the button once there is text: **Steer** folds the message into the turn
// in flight ("also rename the test"), **Follow-up** starts the next turn once this
// one finishes ("then run the tests"). The button says which one it will do and
// the footnote hint follows the toggle. Nothing is added to the transcript
// optimistically: a steer becomes a user turn only when the loop folds it in
// (`steer_applied`), which is where the model actually saw it.
//
// Queued messages appear in a panel **above** the box, with their kind, draft and
// attachment chips, each with **Recall**. Recalling takes the message out of the
// queue (`DELETE /api/sessions/:id/queues/:id`) so it never reaches the model, and
// puts the form back in the composer: the draft you typed, not the rendered body,
// and the chips with their inlined text — so sending again produces exactly the
// request the recall took back. A draft written since the message was queued is
// kept, with the recalled text appended below it. If the run already applied the
// message the server answers `404`: the panel says so and the session is
// refetched rather than reviving something the model has seen.
//
// The pending list is part of the session snapshot (`queued_steers`,
// `queued_follow_ups`), so a reload or a second tab shows the same work, and every
// queue mutation arrives as a `queued_messages` frame carrying both lists in full
// — the newest frame wins, so a missed one cannot leave the panel stale.
//
// # Live throughput and context usage
//
// Two readouts answer the questions a long run raises: *is the model actually
// producing anything*, and *how close is this conversation to its window*.
//
// **Throughput badge.** While a turn is doing something — streaming prose,
// streaming reasoning, running a tool or command, or blocked on an approval —
// **each block that is producing text carries its own badge**, `↓ <tokens>
// <rate> t/s`. It rides the reasoning block's header, the streaming message's
// prose (beside its caret), the tool card's header and the command card's summary
// — never the turn header and never a folded group, which are exactly where a
// long run's reader is not looking. Each badge counts the characters of the block
// it sits on (`chars / 4`, the same ratio `kc-core` budgets with; the tooltip says
// so and names what it counted: prose, reasoning or a call's output), because the
// provider's own `usage` block only arrives once the call is over. The rate is
// averaged over the last ~3 s of *growth*: when a block stops growing its badge
// loses the rate and keeps the count, so a 40 s `cargo test` shows `↓ 12.4k` with
// no invented speed. A block's badge is mounted only while that block works, so
// none of them can outlive the stream they describe. The **Completed** chip still
// uses the turn-wide rule — a turn whose prose finished but whose call is still
// running is **Working**, not **Completed**.
//
// **Usage / context pill.** The composer shows the provider-reported newest prompt
// (`usage.input_tokens`) over the selected model's declared `context_window`, with
// a `Usage` label, percentage and (where it fits) hairline meter. The session model
// pin wins over the install default. Missing provider counts or window metadata
// produce no percentage, never a plausible-looking guess, and unreported metrics
// in Usage stay unknown rather than zero. Session `input_tokens` totals are
// cumulative across calls and are not substituted for prompt fill. The last live
// provider report is cached per session in `karucode.lastPromptTokens` by
// `state/promptTokens.ts`, so a reload can retain that reported figure until the
// next call. The meter turns amber at 70% and red at 90%.
//
// In the ordinary Workbench this pill opens Context → **Usage**. In control-plane
// mode no `onOpenUsage` callback is supplied, so the same real telemetry remains a
// read-only element rather than a control targeting a project drawer that is not
// mounted. At the ≤640px phone tier the decorative meter yields first; the running layout keeps
// Usage with queue/steer while model controls and Stop retain their own row.
//
// # Plugin panels
//
// A plugin can add a tab to the inspector without shipping any UI. `GET
// /api/config` lists the panels ready plugins declare (`plugin_panels`), and the
// inspector appends one tab per panel after the built-in four. Opening a tab
// reads `GET /api/plugins/:name/panels/:panel` scoped by the current project and
// session.
//
// The payload is **untrusted JSON** and is drawn one of three ways:
//
// 1. by a renderer registered for that panel — a first-party component in the
//    app's own bundle, never plugin code;
// 2. as read-only JSON, when no renderer is registered or the renderer declines
//    the payload;
// 3. as a readable error, when the read failed, with a **Try again** button.
//
// To give a plugin a nicer view, register a renderer at startup:
//
// ```ts
// import { registerPluginPanelRenderer } from './components/pluginPanels';
//
// registerPluginPanelRenderer('plugin__kc-todo__todo', {
//   matches: (data) => Array.isArray((data as { items?: unknown })?.items),
//   render: ({ data, refresh }) => <TodoTab data={data} onRefresh={refresh} />,
// });
// ```
//
// `matches` gets `unknown`; validate before reading a field. The registry
// prefers a renderer registered for the hosted id (`plugin__<plugin>__<id>`) and
// falls back to the bare id, so one plugin's panel can be styled without
// affecting another's panel of the same name.
//
// A plugin with no first-party renderer still gets a real view if it uses the
// **`view` vocabulary** (`components/PluginViews.tsx`): a payload shaped like
// `{"view": "stat" | "list" | "bars" | "progress" | "clock" | "stack"}` is claimed
// by shape rather than by id and drawn with the app's own components. Every field
// is text or a number, each is clamped, and a payload that does not match keeps the
// JSON view. With `refresh_ms` set on the panel, a `clock` really ticks — the only
// real-time surface a plugin has, and enough for a timer or a build status.
// `plugins/demos/` has two complete examples (a writing desk that hides chrome, and
// a workspace map with two panels and its own state).
//
// Panel data must never reach `dangerouslySetInnerHTML`, must never be used to
// build a URL, and must never be evaluated: that is the whole point of shipping
// data instead of markup, and it is why the page keeps `frame-ancestors 'none'`
// and a CSP with no `unsafe-eval`. The generic JSON view follows the same rule —
// a string that looks like markup is shown as text.
//
// A browser cannot reach a plugin any other way: there is no plugin route
// mounting, no asset serving and no script execution. A panel is a tab whose
// *contents* the app draws from the plugin's data.
//
// # Plugin controls: buttons, commands, shortcuts and hides
//
// Panels are tabs; `ui` contributions are the rest of the chrome. A plugin
// declares a **button** for a named slot, a **command** for the palette, a
// **keybinding**, or a **hide** for one of the app's own controls, and the app
// draws (or binds) every one of them itself. `GET /api/config`'s `plugin_ui`
// carries the contributions, the hidden targets (with the plugins that asked) and
// the catalogues of slots and hideable names; `components/uiContributions.ts`
// re-validates all of it — an unknown slot, a mismatched hosted id, a control with
// two behaviours or a chord the app already uses is refused, not rendered.
//
// The slots this build mounts:
//
// | Slot | Mount point |
// | --- | --- |
// | `composer.toolbar` | the composer's action row (`Composer`) |
// | `inspector.header` | beside **Context** in the results dock (`Inspector`) |
// | `session.header` | before the built-in chips in the session header (`ChatView`) |
//
// Commands become ordinary palette rows under the **Plugins** group, so filtering,
// arrow keys and Enter treat them exactly like a built-in one — and a row whose
// action is also bound to a chord shows the chord as its hint. A contributed
// control always carries a dot and a tooltip naming its plugin: a plugin must not
// be able to pass its button off as part of the app.
//
// A **keybinding** has its own window listener in `app.tsx`, separate from the
// app's own shortcuts. The two sets cannot collide by construction (a binding must
// carry a modifier, and the app's chords are both host- and client-reserved), and
// a binding never fires while the caret is in a field or while a dialog is open —
// `Ctrl+Shift+F` in the composer belongs to the editor, not to a plugin.
//
// Activating a control has two paths, and `state/actions.ts` owns both:
// `message` sends that text as a user turn (queued behind a running run, like the
// composer), while `action` posts to `POST
// /api/plugins/:name/ui/actions/:action[?project=&session=]` and shows the
// plugin's answer as a toast, re-reads the panels it named, and opens the one
// `http`/`https` URL it may return. Neither feeds the model; a failed action is
// shown as a failure and never as a success.
//
// Hiding is app-wide and allowlisted — `composer.attach`, the four inspector
// tabs, and the inspector's **Context** button. Nothing that stops a run or
// answers an approval is on the list, and the inspector refuses to hide its last
// remaining tab: it keeps the tab strip non-empty and moves the selection to a
// tab that still exists.
//
// The person looking at the screen has the last word: Settings → Plugins & MCP →
// **Controls in the interface** (`components/PluginControls.tsx`) lists every
// contribution per plugin and every hidden built-in with the plugin that hid it,
// with one switch each. The overrides live in `localStorage`
// (`state/pluginUiPreferences.ts`), are applied as a pure transform in
// `applyUiPreferences`, and need no server round trip and no approval — they are a
// preference about this browser, like the theme. A workspace with no
// UI-contributing plugin renders exactly as it did before any of this existed, and
// a slot with no contributions draws nothing at all, not an empty row.
//
// # Keyboard shortcuts
//
// | Keys | Action |
// | --- | --- |
// | `Ctrl/Cmd+K` | command palette |
// | `Ctrl/Cmd+N` | new conversation (a draft until its first message) |
// | `Ctrl/Cmd+B` | collapse/restore the task rail; collapsed desktop keeps project peeking |
// | `Ctrl/Cmd+J` | open/close Workbench Context (Now / Usage / Access), outside the control plane |
// | `Ctrl/Cmd+Shift+T` | open or close the Theme studio |
// | `/` | focus the session search |
// | `Up`/`Down` | move through the `@`-mention suggestions |
// | `Enter`/`Tab` | insert the highlighted mention |
// | `Escape` | dismiss the local editor, menu, drawer or sheet first; only otherwise stop a running turn |
//
// # Mobile
//
// Below 1024 px Sessions and Results retain their full-height slide-over sheets.
// There is no bottom toolbar or ordinary-layout project rail: the sessions sheet
// carries project selection/management, New session, Control plane and Settings;
// the session header retains Sessions, Context, notifications, commands and Task
// actions. Results are also reachable from the command palette and Context's real
// result links. A narrow Source control or Terminal view uses the available
// workspace rather than leaving another result stack beside it.
//
// Context becomes the top full-height dialog, traps focus and makes the mounted
// result sheet inert behind it. Closing it restores the sheet that was underneath
// for that open/close pair instead of inventing a Results selection. Conversation
// and result links explicitly navigate to their destination. Local Escape handlers
// dismiss search/menu/editor state first, then Context or the open sheet; stopping
// a run is not the first effect of dismissing those controls.
//
// The composer remains opaque and measured above the transcript. Its input uses a
// viewport-aware cap and re-grows after resize; at the ≤640px tier its 16px text
// avoids iOS focus zoom. Running controls split deliberately rather than dropping Usage,
// Stop or queue/steer. `viewport-fit=cover` plus the safe-area insets keeps the
// frame clear of the phone status bar and home indicator. Shortcut hint prose and
// decorative meters yield before visible actions and accessible labels.
//
// **The conversation owns the frame, never its content.** The Workbench's Results
// split is a row, so the conversation pane is a flex item in it; left on the
// default `min-width: auto` it took its *content* width and a single unbreakable
// token in a tool card widened the pane past the screen, putting Stop, Queue and
// Approvals off the right edge. `styles/workbench.css` floors it at `min-width: 0`
// outside the real desktop split, so on a phone the pane is exactly the viewport.
// It has to be pinned, not measured in `scrollWidth`: the shell is
// `overflow: hidden`, so the clipped strip never shows up as overflow, and the
// scroller was widened with it and so never scrolled either.
//
// **The keyboard does not cover the composer.** `index.html` asks for
// `interactive-widget=resizes-content` (Android resizes the layout viewport), and
// `lib/viewportInset.ts` publishes the covered strip as `--kb-inset` from
// `visualViewport` for engines that do not - iOS Safari paints the keyboard over
// the page. `styles/layout.css` subtracts it from the shell height, so the dock
// stays above the keyboard. A strip smaller than 15% of the layout is the browser
// toolbar collapsing, not a keyboard, and a pinch-zoomed page is not one either;
// both are ignored rather than shrinking the workspace.
//
// **Touch targets are finger-sized, on every pointer-coarse device.** The header
// controls are 26-31px boxes 4px apart and the dock's attach/Stop/Send were
// 24-28px; `@media (pointer: coarse)` raises them to 40x40 (labelled pills to 32px
// tall) in `styles/workbench.css`, `styles/layout.css` and `styles/composer.css`.
// The workbench header's floor lives in `workbench.css` because that file both owns
// the 30px declaration and loads last. The transcript's row toggles keep their
// density: they are full-width rows, not thumb-sized buttons.
//
// ## Menus on a phone
//
// Every row that offers a menu does so through `lib/menu.tsx`'s `contextMenuProps`.
// Right-click is the desktop gesture and `contextmenu` never fires on iOS Safari,
// so the primitive owns the touch path too: resting a finger for 450ms opens the
// same menu at the same point, travelling more than 10px turns it into a scroll
// and cancels, and the click a browser synthesises afterwards is swallowed so the
// row's own action does not also run. Surfaces still pass `allow` to decline while
// a row is mid-rename. Actions that exist *only* in such a menu - copying a
// commit, a path or an agent id - are therefore reachable without a mouse, and
// `Shift+F10` reaches them from the keyboard.
//
// # Development loop
//
// ```bash
// cd web
// npm ci
// npm run dev        # Vite on :5173, proxies /api to :8787
// npm test           # vitest reducer/component tests
// npm run typecheck
// npm run build      # emits web/dist, embedded on the next cargo build
// ```
//
// Run the backend separately (`cargo run -p kc-server`) while `npm run dev` proxies
// HTTP, SSE and the terminal WebSocket to it (the preview proxy does the same). To
// serve your build without rebuilding the binary, start the server with
// `KARUCODE_WEB_DIR=web/dist`.
//
// For the focused real-browser Workbench check, first run `npm run build` in `web/`
// and build `target/debug/karucode` with `cargo build -p kc-server --bin karucode`.
// Then, from the repository root, run:
//
// ```bash
// python3 scripts/verify_workbench_frontend.py
// ```
//
// It drives Chromium against the built UI and a real debug server with a temporary
// mock provider, disposable git project/data directory and browser profile, not a
// live workspace. This supplements rather than replaces the frontend checks and
// product/backend suites; `web/README.md` lists the coverage and prerequisites.
//
// The UI ships its own type: `web/public/fonts/` carries Inter regular/bold and
// JetBrains Mono, declared with `@font-face` in `styles/base.css` and pointed at
// by `--font-ui` / `--font-mono`. Nothing is fetched from a CDN, so the workspace
// renders identically offline; replacing a file there is the whole procedure for
// changing the UI typeface.
//
// # State and events
//
// The UI is Preact + signals. It bootstraps from `/api/health` and `/api/config`,
// then follows one SSE stream (`/api/events`) and reduces events into the store
// (`src/state/eventReducer.ts`). Session events do not use a general WebSocket;
// the only WebSocket is the terminal's attach socket. Polling is the 20 s health
// check plus the explicitly configured plugin-panel refresh intervals.
//
// **One stream serves every tab of the browser**, not one per tab: the first tab
// takes a Web Lock (`karucode.event-stream`), opens `/api/events`, and relays
// every frame - events, connection status, gaps and resync requests - to its
// siblings over the `karucode.events` BroadcastChannel
// (`src/api/sharedStream.ts`). That is what leaves the browser's
// six-connections-per-origin budget for ordinary requests: with a stream per tab,
// six open tabs held the whole pool and the seventh could not boot (`Could not
// reach karucode - the server did not answer within 30s` on `/api/health`) while
// the server was answering in 1.6 ms. When the leading tab closes or is discarded
// its lock is released and the next waiting tab opens the stream. A browser
// without Web Locks or `BroadcastChannel` - and the test environment - falls back
// to a stream per tab.
//
// The stream watches itself so live updates never need a page reload: 45 s without
// a byte (keepalives included) means the connection is dead and it reconnects with
// backoff; a jump in `seq` or a `stream_lagged` marker means frames were lost, so
// the open session is refetched automatically. The refetch always runs - that is
// how a stuck "running" flag clears when the terminal event was the lost one - but
// it only *replaces* the transcript when the server says no run is in flight, so an
// answer that is still streaming is never deleted.
//
// # Gotchas
//
// * The UI embedded in a release binary is a compile-time snapshot. Rebuilding the
//   frontend alone changes nothing until `cargo build` runs again, unless you set
//   `KARUCODE_WEB_DIR`.
// * The project rail's icon comes from `GET /api/projects/:id/favicon`, fetched
//   with the API client rather than drawn from an `<img src>`: an `<img>` cannot
//   send `Authorization`, and only `/api/events` accepts `?token=`. The bytes
//   become an object URL per project, so adding the same project twice does not
//   duplicate a request, and a project with no favicon answers `204` once per
//   page load and then draws initials. That `blob:` URL is why the page CSP
//   allows `blob:` in `img-src`; remote images stay blocked. There is no manual
//   "choose an icon" setting.
// * The API is same-origin: the dev server proxies `/api`, so run the backend
//   alongside it instead of pointing the browser at two origins.
// * `Ctrl/Cmd+N` is intercepted only while no dialog is open; inside the composer
//   the browser default is left alone where it matters.
//
// Full reference: `README.md` (Development); panel and shortcut details are in
// the generated `docs/internal/web-ui.md`.

import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import * as actions from './state/actions';
import * as store from './state/store';
import * as workbench from './state/workbench';
import * as notifyWiring from './state/notifyWiring';
import { AuthScreen } from './components/AuthScreen';
import { BootErrorScreen, BootSkeleton } from './components/BootScreens';
import { ChatView } from './components/ChatView';
import { CommandPalette } from './components/CommandPalette';
import { ErrorBoundary } from './components/ErrorBoundary';
import { FileViewer } from './components/FileViewer';
import { NotificationCenter } from './components/NotificationCenter';
import { ProjectRail } from './components/ProjectRail';
import { ControlPlane } from './components/control-plane/ControlPlane';
import { controlPlaneOpen } from './state/controlPlane';
import * as railPeek from './state/railPeek';
import { RewindDialog } from './components/RewindDialog';
import { SessionsRail } from './components/SessionsRail';
import { SettingsSheet } from './components/SettingsSheet';
import { matchUiKeybinding } from './components/uiContributions';
import { ThemeStudio } from './components/ThemeStudio';
import { ToastRegion } from './components/ToastRegion';

function isDialogOpen(): boolean {
  return (
    store.settingsOpen.value ||
    store.paletteOpen.value ||
    store.themeStudioOpen.value ||
    store.notificationCenterOpen.value ||
    store.rewindTarget.value !== null ||
    store.fileViewerPath.value !== null ||
    (store.narrowViewport.value && workbench.contextOpen.value)
  );
}

export function App(): JSX.Element {
  useEffect(() => {
    const unregisterAuth = actions.registerAuthHandler();
    void actions.bootstrap();

    // Sound needs one interaction before a browser will let it play. Attaching the
    // listener is all this does; it removes itself after the first gesture.
    const disarmSound = notifyWiring.armSoundOnFirstGesture();
    // Keep the engine in step with the persisted preferences, and with a change
    // made in another tab.
    notifyWiring.syncSoundPreferences();
    const onStorage = (event: StorageEvent): void => {
      if (event.key === null || event.key.startsWith('karucode.notify.')) {
        store.soundEnabled.value = store.readSoundEnabled();
        store.soundVolume.value = store.readSoundVolume();
        store.desktopEnabled.value = store.readDesktopEnabled();
        notifyWiring.syncSoundPreferences();
      }
    };
    window.addEventListener('storage', onStorage);

    const clockTimer = window.setInterval(() => {
      store.clock.value = Date.now();
    }, 20_000);
    const healthTimer = window.setInterval(() => {
      void actions.checkHealth();
    }, 20_000);

    const media =
      typeof window.matchMedia === 'function' ? window.matchMedia('(max-width: 1023px)') : null;
    const updateViewport = (): void => {
      store.narrowViewport.value = media ? media.matches : false;
      if (media && !media.matches) store.mobileSheet.value = null;
    };
    updateViewport();
    if (media) {
      if (typeof media.addEventListener === 'function') media.addEventListener('change', updateViewport);
      else if (typeof media.addListener === 'function') media.addListener(updateViewport);
    }

    return () => {
      unregisterAuth();
      disarmSound();
      window.removeEventListener('storage', onStorage);
      window.clearInterval(clockTimer);
      window.clearInterval(healthTimer);
      if (media) {
        if (typeof media.removeEventListener === 'function') media.removeEventListener('change', updateViewport);
        else if (typeof media.removeListener === 'function') media.removeListener(updateViewport);
      }
      actions.stopStream();
    };
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      if (store.boot.value !== 'ready') return;
      const mod = event.metaKey || event.ctrlKey;
      const key = event.key.toLowerCase();

      if (mod && event.shiftKey && key === 't') {
        event.preventDefault();
        actions.toggleThemeStudio();
        return;
      }
      if (mod && key === 'k') {
        event.preventDefault();
        actions.togglePalette();
        return;
      }
      // The Theme Studio owns no key handler of its own, so Escape must close it
      // before the generic "a dialog is open, ignore shortcuts" guard below.
      if (event.key === 'Escape' && store.themeStudioOpen.value) {
        actions.closeThemeStudio();
        return;
      }
      if (event.key === 'Escape' && workbench.contextOpen.value && !event.defaultPrevented) {
        event.preventDefault();
        workbench.closeContext();
        return;
      }
      if (isDialogOpen()) return;

      // The collapsed rail's fly-out is not a dialog and has no focus trap, so it
      // closes itself here — before the Escape that stops a run, which would
      // otherwise cancel a run when all the reader wanted was to dismiss the list.
      if (event.key === 'Escape' && railPeek.peekProjectId.value !== null) {
        railPeek.closePeek();
        return;
      }

      if (mod && key === 'b') {
        event.preventDefault();
        // A peek is a stand-in for the rail: collapsing or restoring it is over.
        railPeek.closePeek();
        actions.toggleRail();
        return;
      }
      if (mod && key === 'j') {
        event.preventDefault();
        if (!controlPlaneOpen.value) workbench.toggleContext();
        return;
      }
      if (mod && key === 'n') {
        // Unlike the other shortcuts this one starts a new conversation, so it
        // must not fire while the user is typing: `Ctrl+N` is a text-editing key
        // in the composer and in every inline editor.
        const target = event.target;
        if (
          target instanceof HTMLElement &&
          target.closest('input, textarea, select, [contenteditable="true"]')
        ) {
          return;
        }
        event.preventDefault();
        actions.startDraftSession();
        return;
      }
      if (event.key === 'Escape') {
        const target = event.target;
        // The composer handles Escape itself; inline editors and open menus
        // consume it so closing them does not also cancel the run.
        if (
          target instanceof HTMLElement &&
          target.closest('input, textarea, select, [contenteditable="true"], [role="menu"]')
        ) {
          return;
        }
        if (store.mobileSheet.value !== null) {
          actions.closeSheets();
          return;
        }
        if (store.chat.value.running && !store.stopping.value) {
          void actions.stopRun();
        }
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  /**
   * Shortcuts plugins contributed.
   *
   * A separate listener from the app's own, on purpose: the two sets cannot
   * collide (a plugin binding must carry a modifier, and the host and the client
   * both refuse the app's own chords), so neither has to know about the other, and
   * a plugin that vanishes mid-session simply has no bindings in the next read.
   *
   * Two guards matter. No binding fires while a dialog is open — a shortcut that
   * acts behind a modal is a bug. And none fires while the caret is in a field: a
   * chord with a modifier is still a text-editing key there (Ctrl+A, Ctrl+Z), and
   * a plugin must not make typing surprising.
   */
  useEffect(() => {
    const onPluginKeyDown = (event: KeyboardEvent): void => {
      if (store.boot.value !== 'ready') return;
      if (event.defaultPrevented || isDialogOpen()) return;
      const target = event.target;
      if (
        target instanceof HTMLElement &&
        target.closest('input, textarea, select, [contenteditable="true"]')
      ) {
        return;
      }
      const binding = matchUiKeybinding(store.pluginUi.value.keybindings, event);
      if (!binding) return;
      event.preventDefault();
      void actions.runPluginContributions(binding);
    };
    window.addEventListener('keydown', onPluginKeyDown);
    return () => window.removeEventListener('keydown', onPluginKeyDown);
  }, []);

  useEffect(() => {
    if (controlPlaneOpen.value) {
      workbench.closeContext();
      store.mobileSheet.value = null;
    }
  }, [controlPlaneOpen.value]);

  const session = store.activeSession.value;
  const unreadCount = store.unreadTotal.value;
  useEffect(() => {
    const base = session?.title ? `${session.title} — karucode` : 'karucode';
    // The unread count is the only thing a notification can tell you from another
    // tab, so it goes in the title rather than only in the page.
    document.title = unreadCount > 0 ? `(${unreadCount}) ${base}` : base;
  }, [session?.title, unreadCount]);

  if (store.boot.value === 'loading') return <BootSkeleton />;
  if (store.boot.value === 'auth') return <AuthScreen />;
  if (store.boot.value === 'error') return <BootErrorScreen />;

  return (
    <div class="app-root">
      <a class="skip-link" href="#chat-main">
        Skip to chat
      </a>
      {/* One boundary per panel.
        *
        * The app has two layers at least: this outer shell boundary and one per
        * grid child or overlay. A malformed payload in the inspector, a plugin
        * panel that returns something its renderer did not expect, a theme file
        * with a corrupt token - each throws inside one component, and without a
        * boundary around it the throw unmounts the whole tree and leaves a white
        * page with no chat, no rails and no way back but the browser's reload.
        *
        * `ChatView` is deliberately NOT wrapped here: it carries its own
        * boundaries for the transcript and each of its sub-panels, so a crash in
        * the conversation does not also take the composer down.
        */}
      <ErrorBoundary scope="The workspace" root>
        <div
          class={`app${store.narrowViewport.value ? ' is-narrow' : ''}`}
          data-rail-collapsed={store.railCollapsed.value ? 'true' : 'false'}
          data-inspector-collapsed={store.inspectorCollapsed.value ? 'true' : 'false'}
        >
          {controlPlaneOpen.value || (!store.narrowViewport.value && store.railCollapsed.value) ? (
            <ErrorBoundary scope="The project rail">
              <ProjectRail />
            </ErrorBoundary>
          ) : null}
          {controlPlaneOpen.value ? (
            <ErrorBoundary scope="Mission control"><ControlPlane /></ErrorBoundary>
          ) : (
            <>
              <ErrorBoundary scope="The sessions rail"><SessionsRail /></ErrorBoundary>
              <ChatView />
            </>
          )}
        </div>
      </ErrorBoundary>

      {store.mobileSheet.value !== null ? (
        <button
          type="button"
          class="sheet-backdrop"
          aria-label="Close panel"
          onClick={actions.closeSheets}
        />
      ) : null}

      <ErrorBoundary scope="Settings">
        <SettingsSheet />
      </ErrorBoundary>
      <ErrorBoundary scope="The command palette">
        <CommandPalette />
      </ErrorBoundary>
      <ErrorBoundary scope="The Theme studio">
        <ThemeStudio />
      </ErrorBoundary>
      <ErrorBoundary scope="Notifications">
        <NotificationCenter />
      </ErrorBoundary>
      <ErrorBoundary scope="The rewind dialog">
        <RewindDialog />
      </ErrorBoundary>
      <ErrorBoundary scope="The file viewer">
        <FileViewer />
      </ErrorBoundary>
      {/* The toast region stays OUTSIDE every boundary on purpose: it is how a
        * crash gets reported to the user ("could not save", "the stream died"),
        * so replacing it with a crash card would hide the very messages it exists
        * to deliver. It renders nothing but text and has no dependencies, so it
        * has nothing to throw about. */}
      <ToastRegion />
    </div>
  );
}