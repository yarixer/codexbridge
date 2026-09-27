# CodexBridge: Plan for Moving from Prototype to Working Product

Date: September 13, 2026. Baseline: v0.2.0, commit `42caff2`.
Status: development plan. The new capabilities described below have not yet been implemented.

## 1. Goal and Scope

Create a desktop application for Windows and Linux with the interface and behavior of Cursor's agent functionality: projects, persistent chats, live agent actions, models, Rules, Skills, search, settings, and result review and acceptance. The name is CodexBridge.
Primary workflow: select or create a repository → write a message → watch the agents work → review the result → apply the changes → continue the conversation. Connections are configured once.

The baseline profile remains unchanged: GPT-6 Astra through a ChatGPT/Codex subscription plans and reviews, while Grok 4.6 xHigh Fast through the Cursor SDK implements. The roles remain the same when other available models are selected. Astra is not permitted to modify project code.

The visual reference is the [Cursor Agents Window](https://cursor.com/docs/agent/agents-window). We will carry over its composition, density, navigation, chat behavior, and agent controls. CodexBridge will have its own name and icon. The IDE, extensible code editor, debugger/LSP, and MCP are excluded. Viewing files, diffs, images, and command output remains part of the agent result. A simple text editor for Rules/Skills is part of settings.

Cloud VMs, SSH, Marketplace, team accounts, and a separate synchronization server are not included in the first full version. In this plan, "like Cursor" means the listed agent workflows for local repositories.

## 2. What Has Already Been Proven and What Must Be Replaced
We are keeping Rust/Tauri, provider integrations, Git worktrees, and SQLite. We are redesigning the application screen, chat model, run lifecycle, and event delivery.

| Current State | Target Behavior |
|---|---|
| The home page is diagnostics and a task form | The home page is projects and chat; diagnostics are inside settings |
| One restored most recent TaskSession | Projects, multiple chats, message history, and separate runs within a chat |
| Cursor `server_stream` reads the entire `response.bytes()` | Incremental frame parsing and immediate event display |
| `enableDeltas` and `enableSteps` are disabled in the request | The adapter selects and reconciles available text events and steps without duplication |
| Astra creates and deletes a temporary thread | A persistent architect session with managed context compaction |
| Models and effort are hard-coded | Provider catalog and saved settings by role/project/chat |
| The Cursor key is stored only in memory | System secret storage and automatic restoration |
| Retries are counted ambiguously | Separate accounting for runs, transport retries, correction iterations, and reviews |
| A lack of visual evidence means "needs rework" | A separate "evidence required" reason and an action to obtain it |
| The result remains at a long AppData path | File list, open/run, diff, and apply to the selected branch |

Basis: `src/App.tsx`, `src-tauri/src/lib.rs`, `crates/codexbridge-core/src/{cursor,codex,orchestrator,store,validation,git}.rs`, and the results of the user test already conducted. The current successful Astra → Grok → checks → Astra exchange proves that the integrations work, but not that the interface is ready for daily use.

## 3. Interface

### Layout

| Area | Content |
|---|---|
| Resizable left sidebar | New chat, search, projects; a list of chats with statuses inside each project; pinning, archive; Rules, Skills, Settings |
| Top bar | Chat title, project/branch, operating mode, run status; actions menu |
| Center | A unified feed: user, Astra, Grok, system messages, commands, changes, checks, and reviews |
| Bottom composer | Multiline input, attachments, `@` context, `/` skills; mode, models, effort, Fast, context; send/stop |
| Collapsible right panel | "Plan," "Changes," "Results," and "Checks" tabs; file/diff/image preview |

Settings and connections do not occupy half of the screen permanently. An empty chat invites the user to enter a task. The result folder and current stage are always visible for a task. Astra's status does not disappear if it has not yet begun its review.

### Themes and Visual Acceptance

Light, dark, and system themes; the selection is persisted. Shared tokens for colors, typography, spacing, borders, hover/focus/disabled states, and statuses. The dark theme uses a neutral, nearly black background; the light theme uses a white background with distinct panel shades. Initial layout dimensions: sidebar 240–300 px, right inspector 320–420 px, body text 13–14 px; refine them by comparison with the reference.

Before implementing screens, capture references from a single Cursor version: empty project, active chat, model selection, settings, Rules/Skills, and diff view. Prepare mockups of the same states for both themes. Use small controls, moderate corner radii, compact action rows, and readable Markdown messages. Compare not only colors, but also geometry, density, and scrolling behavior.

Test at 1280×800 and 1920×1080, with Windows scaling at 100/125/150%, keyboard focus, contrast, and no horizontal overflow. Every visible button must either work or have a clear reason for being unavailable.

### Chat and Workflow

Messages continue the existing conversation; the title can be renamed, and the chat can be pinned, archived, and found through search. Switching chats does not stop a run. Long histories load in chunks.

The feed shows: who is working and with which model; streaming text; available reasoning summaries/thinking events; file reads and searches; command invocations and their output; changed files; plan items; task handoff from Astra to Grok; test results; and review output. These entries are collapsible and include a timestamp, status, and file links.

"What the agent is thinking" means explanations and reasoning events actually emitted by the provider. The interface does not promise access to the complete hidden chain of thought. When no text is available, it shows the honest status "Thinking" without an explanation generated by another model.

Sending while work is in progress: a message queue, removal of a queued message, and clarification of the current run when supported by the provider. If a particular adapter does not support live steering, the message remains in the queue with an explicit label. The Stop button actually cancels the run; Escape closes a menu without accidentally canceling work.

## 4. Repositories and Results

Three actions on the start screen: open an existing folder/repository, clone by URL, or create a new folder with Git. For a new Git repository without HEAD, the required initial baseline is created as part of the explicitly selected creation workflow. The folder's existence, write permissions, Git, and selected branch are checked.

Store the list of recent and pinned projects, path, branch, and settings. For a dirty checkout, offer a choice of baseline: the latest commit or a separate snapshot of the current changes. Do not force the user to stash manually before every chat. By default, the agent works in a separate worktree associated with the chat; subsequent messages use the same working copy. Repositories with spaces and Cyrillic characters in their paths are included in mandatory tests.

Results are shown as soon as they appear: file, change type, size, open folder, copy path, and text/image preview. Running a program is represented as a saved action with a command, cwd, and log; commands are not guessed without a confirmed run recipe.

Changes panel: diff by file and for the entire task, added/deleted/renamed and binary files, and checkpoints. "Apply result" shows the target branch and the complete change set, then integrates the selected verified version. If there is a conflict, show a clear list and offer to have Grok resolve it in a separate working copy. The user's uncommitted changes are preserved. Partial acceptance invalidates the verdict for the previous full diff and requires verification of the selected set.

Worktree cleanup is a separate action after applying/archiving, with a check for unapplied results. Archiving a chat does not mean deleting its files.

## 5. Key, Connections, and Settings

Persisting the Cursor API key is the first mandatory deliverable. Use Credential Manager on Windows and Secret Service on Linux; the Rust adapter provides `save/read/delete/status`. The key remains in the backend; the UI receives connection status and a masked value. Reopening the application does not require re-entering the key. The key can be cleared/replaced in settings. An empty diagnostics field must not delete the saved secret.

On Linux, distinguish between "key is absent" and "secret store is locked/not installed." For environments without Secret Service, provide an explicitly selected encrypted store with an unlock mechanism; never silently fall back to plaintext. SQLite, localStorage, chat exports, and logs do not contain the secret.

Astra continues to use ChatGPT/Codex sign-in. Account status is read automatically; the login process belongs to CodexBridge. The initial check runs after setup, then runs in the background and from a button in settings. Projects, chats, and results can be viewed without both providers being connected. Continuing validation/review does not require a Cursor key if Grok is not invoked.

Settings sections: General, Appearance, Accounts, Models, Context, Orchestration, Rules, Skills, Projects, Validation Commands, Storage, and Diagnostics. Scopes: global defaults → project → chat; a snapshot of the effective values is recorded for every run. Changes made while work is in progress take effect on the next turn.

## 6. Models, Effort, Persistence, and Context

| Setting | Semantics |
|---|---|
| Architect model | GPT-6 Astra by default; available alternatives from the Codex model catalog; read-only role |
| Implementer model | Grok 4.6 by default; other models only from the available Cursor catalog |
| Reasoning depth | Values actually provided by the selected model; `xHigh` is not imposed on every model |
| Fast | A separate parameter/variant, only when present in the catalog |
| Persistence | CodexBridge policy: maximum correction cycles, working time, stop conditions, and lack of progress |
| Context | Budget for supplied context, and policies for history retention, compaction, and file retrieval |

Proposed persistence profiles: one iteration; standard—up to three correction cycles; persistent—up to six; custom limits. These numbers are proposed product defaults. A network reconnect and collection of missing evidence do not count as a new correction cycle. Repeating the same error without a state change produces a specific request for help instead of an infinite retry.

Context: Auto and a manual budget, such as 32k/64k/128k, only within the selected model's permitted range. The UI distinguishes between "the budget assembled by CodexBridge" and "the provider's actual limit/mode." A slider does not increase the model's context window. If the SDK does not provide a hard limit for total context, state this and apply the setting to managed attachments and summaries. Unknown usage is labeled unknown; an estimate is labeled approximate.

The composer includes a used/available context indicator; clicking it shows the composition: instructions, Rules, Skills, messages, files, tool results, and summary. The portion hidden inside the provider is marked as unavailable for breakdown. Context pinning and "Compact history" are available. After compaction, preserve the goal, decisions made, constraints, unfinished actions, and links to evidence.

Astra usage is reduced through architecture: a focused plan and result review, compact tasks for Grok, local search and checks without an LLM, a full on-disk log, and only the necessary excerpts for review. A truncated diff is not presented as complete: split it by file and explicitly track unreviewed parts. Make no savings percentage claims without measuring identical tasks.
## 7. Rules and Skills

Rules: create, edit, delete, enable, choose global/project scope, and select the Astra/Grok/both role. Modes: always, by files, manual, and by relevance. For each run, display the rules that were actually active and why they were included.

Native project rules: `.codexbridge/rules/*.mdc`; global rules are in the CodexBridge user directory. Support reading and importing `.cursor/rules/*.mdc`, `AGENTS.md`, and existing legacy rules. Do not rewrite third-party files automatically. The conflict resolution order is fixed and shown to the user; exact task constraints and role policies are not displaced by an imported rule.

Skills: catalog, search, import from a local folder or Git URL with a pinned revision, simple `SKILL.md` creation, preview, and enable/disable by scope and role. The canonical compatible format is `SKILL.md` plus optional `scripts/`, `references/`, and `assets/`. Support `.agents/skills/` and importing `.cursor/skills/`. Initially, the agent receives skill names and descriptions; the full text and additional files load as needed. Manual `/skill-name` invocation and a visible list of active skills.

The adapter must prevent duplicate loading of the same Rules/Skills through native discovery and the context assembled by CodexBridge. File-scoped rules must also apply to files discovered during work. If native discovery does not cover the required scope/role, use a controlled provider-specific instruction bundle; verify this with a contract test. Skill capabilities run through the relevant agent's tools, without MCP. A skill that requires an unavailable integration is marked incompatible.
Check import formats and behavior against [Cursor Rules](https://cursor.com/docs/rules) and [Agent Skills](https://cursor.com/docs/skills). Format compatibility does not imply automatic compatibility with every skill action.

## 8. Orchestration That Brings a Task to Completion

Modes: "Discuss," "Plan," and "Execute." The first two do not grant the implementer permission to write files. A request to "give me a brief plan" ends with a response in the chat. Implementation starts through an explicit user action or a pre-enabled Agent policy for that chat.

In execution mode: Astra asks necessary clarifying questions and creates a plan → Grok works → local checks → Astra reviews changes and evidence → issues are corrected/evidence is collected → final result. After one-time plan approval, the cycle proceeds automatically within the persistence limits and permissions. Approval is requested again only for a material change to the agreed scope or a separate protected action.

Separate states and reasons: awaiting approval, running, validating, review, code changes required, evidence required, user input required, provider limitation, environment error, stopped, completed. In the UI, use readable names, stage, reason, and a specific action; SDK enum strings remain in diagnostics.
An acceptance criterion has an id, check type, required flag, and evidence links. Evidence includes the command/method, cwd, exit code, timestamp, source version, and artifact. Visual verification includes the captured image and verification source. Grok's claim "I looked at it" does not replace an artifact. For a test plot: run the script and save a PNG, attach the PNG and data/a check of the point (0,0) and symmetry, then send it to a reviewer with image support. If the image is unavailable to the adapter, request a specific user action and clearly identify the missing evidence.

The review returns a structured verdict: accepted, code changes required, evidence required, or user response required. A missing screenshot does not trigger a rewrite of working code. An npm/PATH error triggers validation recovery. A transport error triggers run-status reconciliation and reconnect. Astra's result review is read-only; the worker/runner launches a program to obtain an artifact.

Rust executes deterministic rules. Every step has a stable id and journal. Repeated clicks do not create a second run. After a disconnect, first determine the provider's state, then continue work. Only one active modifying run is allowed per worktree; parallel chats use separate working copies.

## 9. Technical Foundation

Keep Tauri 2 + React/TypeScript for the desktop UI and Rust/Tokio for logic. Split the UI by feature instead of keeping it in a single `App.tsx`. Separate Rust modules: projects, chats, runs, providers, events, orchestration, credentials, context, rules, skills, validation, git/results, search, settings.
SQLite with migrations: projects, chats, messages, runs, provider_sessions, run_events, artifacts, acceptance_checks, settings, instruction_snapshots. The API key is not stored in these tables. Old tasks/artifacts migrate into legacy chats while preserving history, paths, and incomplete states. Plan, source, diff, and review versions are linked; an old approval does not apply to new code.

Run management is separated from the UI request: short start/stop/continue commands followed by event subscription through Tauri IPC. Events contain project/chat/run/actor, local sequence, timestamp, type, and schemaVersion. The durable journal is stored before publication; the UI can retrieve events from a sequence number. Frequent text deltas are grouped into small batches to avoid overloading SQLite and rendering.

Cursor adapter: long-lived agent/run handles, incremental Connect parser, durable replay, cancel, catalog, and error reconciliation. Support for `Send`, `ObserveRun`, `GetRun`, and `CancelRun` is documented in [Cursor SDK services](https://github.com/cursor/sdk-bridge/blob/main/docs/services.md). An important recovery detail: the offsets for live `Send` and durable `ObserveRun` are not interchangeable; store them separately and deduplicate during replay. See [streaming semantics](https://github.com/cursor/sdk-bridge/blob/main/docs/streaming.md).

Codex adapter: persistent threads, response/notification routing, catalog, text and reasoning-summary events, cancellation, usage, and session recovery. The contract is based on the installed app-server version; experimental fields are enabled only when support has been verified. Basis: [Codex App Server](https://learn.chatgpt.com/docs/app-server).

Capabilities are queried on connection: models/effort, modes, images, context, usage, cancel/resume/steer, and instruction support. The UI does not present unsupported toggles as functional. For example, local Cursor `GetUsage` is marked cloud-only in the pinned proto: use available run events, and do not interpret missing data as zero usage or no cost.
On Windows: correct EXE/CMD resolution, paths, and process-tree termination; on Linux: executable permissions, PATH, and process groups. A timeout terminates the process rather than only stopping the wait for its result. Closing the UI and exiting the application are distinct: while work is active, the app can minimize to the tray; an explicit exit stops the run while preserving state. After a crash, the app reconciles saved ids with the provider and does not promise to resume a process that has terminated.

A worktree separates task files but is not an OS sandbox. Astra's read-only restriction is enforced at the tool/provider level. Grok's profile shows its actual permissions. The application itself does not connect MCP and must not accidentally launch inherited global MCP configurations; this is a separate test for the provider's isolated profile.

Search: SQLite FTS for messages and titles, plus a local filename index/repository text search with ignore rules. Full logs and large artifacts are stored on disk with metadata in the database. Search and UI construction do not invoke Astra.

## 10. Implementation Stages and Acceptance

Each stage ends with a runnable result and scenario verification. A new build includes its version/commit in About. Visual quality is part of a stage's definition of done rather than something deferred until the end.
| Stage | Work | Verifiable Result |
|---|---|---|
| M0. Connections and persistence | Credentials, automatic settings load, background checks, capability probes, regression scenarios for known errors | The key is not re-entered after three restarts; the model catalog loads; validation continues without a Cursor key; history is available offline |
| M1. New UI and themes | Cursor references, design tokens, sidebar/chat/composer/inspector, settings, light/dark/system themes | Empty and active chats, settings, and diff look consistent in both themes; theme and panel state persist |
| M2. Projects and persistent chats | Open/clone/create, data schema and migrations, chat list, archive, title search, recovery | Create a new repo, add an existing one, and clone a third; open two chats in each, restart, and continue the correct chat with the same working folder |
| M3. Real-time work | Event journal, streaming from both providers, Markdown/tool cards, stdout, cancel, queue, reconnect | See actions before the final answer; switch chats during a run; actually cancel it; after a disconnect, do not get a second run or duplicate messages |
| M4. Models and context | Role-based picker, effort/Fast, persistence, budgets/compaction, usage, `@` attachments, and scoped settings | Selected parameters appear in the provider request snapshot; unsupported values are excluded; a long chat is compacted without losing the goal; estimates are labeled |
| M5. Rules and Skills | CRUD, compatible-file import, scopes and roles, activation, slash menu, instruction snapshots | A rule actually changes behavior in its scope; a disabled rule is not applied; a skill is invoked manually/by relevance; settings remain after restart |
| M6. Complete workflow | Discuss/Plan/Agent modes, automatic corrections, evidence, review, diff/artifacts, apply, checkpoints | A question gets an answer without edits; a defect is returned to Grok; a missing image is collected separately; an accepted result is transferred to the required branch without losing user files |
| M7. Search, quality, and release | Full-text search, palette/shortcuts, history performance, Windows/Linux installer tests, release builds, documentation | All scenarios below pass in the installed application on both operating systems; there is no dependency on the developer's cwd or an open PowerShell session |

Sequence: M0 → M1 → M2 → M3 → M4 → M5 → M6 → M7. If a capability probe discovers an SDK limitation, document it before implementing a dependent control. Refine time estimates based on M0–M1 results; completion is tied to verification, not the number of screens written.

## 11. Mandatory v1 Readiness Checks

1. The key and theme survive a restart; connection setup is not a daily form.
2. Open, clone, and create repository work, including a new repo without commits and a dirty checkout.
3. Chats and artifacts from the previous v0.2 remain available after migration.
4. "Give me a brief plan" does not modify files or launch Grok automatically.
5. In Agent mode, Astra plans, Grok implements, and the user sees real events from both before the run ends.
6. Stop cancels execution, repeated clicks do not duplicate the run, and the correct stage is restored after a failure.
7. Model, effort, Fast, persistence, and context are either actually applied or clearly marked as unavailable.
8. Rules/Skills affect the correct project and role; the user sees the active set.
9. A test Python plot reaches review with a real PNG; missing evidence does not require rewriting the code.
10. The user sees where the program was saved, opens the file/folder, views the diff, and applies the final result to the selected repository.
11. CLI, auth, rate-limit, and environment errors include a specific cause and available action; completed does not simply mean "the model stopped responding."
12. Light and dark themes, search, keyboard use, and a long chat pass visual and functional acceptance.
13. Installed Windows/Linux release builds work from a normal launcher, with paths containing spaces and Cyrillic characters, and without a development environment.

Test strategy: unit tests for the state machine/context/rule resolution; recorded protocol fixtures for split frames, keepalive, duplicate replay, and errors; integration tests with real Git/temporary repositories; UI scenarios using mock providers; limited live account smoke tests; final desktop-installer and theme verification on Windows/Linux. The UI test task does not consume model usage on every CI run.

The first user-facing result of the new plan is a persisted key and a proper shell with projects and chat. The final result is the complete, controlled path from a message to verified and applied files. "Received a response from Astra/Grok" is no longer used as the criterion for product completion.
