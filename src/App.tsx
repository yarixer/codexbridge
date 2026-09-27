import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Archive, ArrowLeft, ArrowRight, ArrowUp, Blocks, Bot, BrainCircuit, Check, CheckCircle2, ChevronDown,
  CircleAlert, CircleDot, Clock3, Copy, FolderGit2, FolderOpen, GitBranch,
  KeyRound, ListChecks, LoaderCircle, Moon, MoreHorizontal, File, Globe, Laptop, ListFilter, Minus, PanelLeft, PanelRight, Square, GitCompareArrows,
  Play, Plus, Search, Send, Settings, ShieldCheck,
  Sun, TerminalSquare, X,
} from "lucide-react";

type HealthState = "ready" | "needsLogin" | "needsApiKey" | "missingDependency" | "unavailable";
type Theme = "system" | "light" | "dark";
type ProviderHealth = { state: HealthState; title: string; detail: string; models: { id: string; displayName: string }[]; usage: unknown | null };
type EnvironmentReport = { codex: ProviderHealth; cursor: ProviderHealth; workspace: string };
type CredentialStatus = { configured: boolean; masked?: string; backend: string; error?: string };
type AppSettings = { workspace: string; theme: Theme; activeProjectId?: string; activeChatId?: string };
type Project = { id: string; name: string; path: string; remoteUrl?: string; createdAt: number; updatedAt: number };
type Chat = { id: string; projectId: string; title: string; archived: boolean; createdAt: number; updatedAt: number; taskId?: string; taskStatus?: string };
type WorkItem = { id: string; objective: string; allowedPaths: string[]; dependsOn: string[]; acceptanceCriteria: string[] };
type TaskSession = {
  task: { id: string; status: string; workerAttempts: number; spec: { goal: string; workspace: string; baseCommit: string; maxWorkerAttempts: number } };
  blockedFrom?: string;
  plan?: { summary: string; workItems: WorkItem[]; risks: string[] };
  worktree?: string;
  worker?: { status: string; text: string; durationMs: number; events: { kind: string; summary: string }[] };
  validation: { command: string[]; success: boolean; exitCode?: number; output: string }[];
  review?: { approved: boolean; summary: string; issues: string[] };
};

const statusLabel: Record<string, string> = {
  planning: "Generating plan", executing: "Working", validating: "Running checks",
  reviewing: "Reviewing", needsRevision: "Ready for review", blocked: "Needs attention",
  completed: "Ready for review", cancelled: "Stopped",
};

function StatusDot({ state }: { state?: HealthState }) {
  return <span className={`status-dot ${state ?? "unavailable"}`} />;
}

function nonEmptyLines(value: string) {
  return value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
}

function commandArgs(value: string) {
  return nonEmptyLines(value).map((line) =>
    Array.from(line.matchAll(/"([^"]*)"|'([^']*)'|([^\s]+)/g), (match) => match[1] ?? match[2] ?? match[3]),
  );
}

function relativeTime(timestamp: number) {
  const elapsed = Math.max(0, Date.now() - timestamp * (timestamp < 1e12 ? 1000 : 1));
  const minutes = Math.floor(elapsed / 60000);
  return minutes < 1 ? "now" : minutes < 60 ? minutes + "m" : minutes < 1440 ? Math.floor(minutes / 60) + "h" : Math.floor(minutes / 1440) + "d";
}

function MessageBody({ children }: { children: string }) {
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState(false);
  return <><div className="markdown"><Markdown remarkPlugins={[remarkGfm]} components={{ a: (props) => <a {...props} target="_blank" rel="noopener noreferrer" /> }}>{children}</Markdown></div><div className="message-actions"><button aria-label={copied ? "Copied" : "Copy response"} title={copied ? "Copied" : "Copy response"} onClick={async () => { try { await navigator.clipboard.writeText(children); setCopied(true); setCopyError(false); } catch { setCopyError(true); } }}>{copied ? <Check size={13} /> : <Copy size={13} />}</button>{copyError && <span role="status">Could not copy. Select the text and press Ctrl+C.</span>}</div></>;
}

export default function App() {
  const [theme, setTheme] = useState<Theme>("system");
  const [projects, setProjects] = useState<Project[]>([]);
  const [activeProject, setActiveProject] = useState<Project>();
  const [chats, setChats] = useState<Chat[]>([]);
  const [activeChat, setActiveChat] = useState<Chat>();
  const [session, setSession] = useState<TaskSession>();
  const [report, setReport] = useState<EnvironmentReport>();
  const [credential, setCredential] = useState<CredentialStatus>();
  const [cursorKey, setCursorKey] = useState("");
  const [goal, setGoal] = useState("");
  const [constraints, setConstraints] = useState("");
  const [acceptance, setAcceptance] = useState("");
  const [validationCommands, setValidationCommands] = useState("cargo test --workspace\nnpm run build");
  const [search, setSearch] = useState("");
  const [searchVisible, setSearchVisible] = useState(false);
  const [sidebarVisible, setSidebarVisible] = useState(true);
  const [toolsVisible, setToolsVisible] = useState(true);
  const [appMenu, setAppMenu] = useState<"File" | "Edit" | "View" | "Help">();
  const [details, setDetails] = useState<"changes" | "terminal">();
  const [diff, setDiff] = useState("");
  const [detailsBusy, setDetailsBusy] = useState(false);
  const [headerMenu, setHeaderMenu] = useState(false);
  const [settingsTab, setSettingsTab] = useState("Agents");
  const [projectMenu, setProjectMenu] = useState(false);
  const [modal, setModal] = useState<"create" | "clone" | "settings">();
  const [modalPath, setModalPath] = useState("");
  const [cloneUrl, setCloneUrl] = useState("");
  const [showArchived, setShowArchived] = useState(false);
  const [busy, setBusy] = useState(false);
  const [workflowBusy, setWorkflowBusy] = useState<"planning" | "executing">();
  const [error, setError] = useState<string>();
  const workflowPending = useRef(false);
  const composerInput = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const input = composerInput.current;
    if (input) {
      input.style.height = "auto";
      input.style.height = Math.min(180, input.scrollHeight) + "px";
    }
  }, [goal]);

  useEffect(() => { document.documentElement.dataset.theme = theme; }, [theme]);

  useEffect(() => {
    let cancelled = false;
    async function restore() {
      setBusy(true);
      try {
        const [savedCredential, settings, savedProjects] = await Promise.all([
          invoke<CredentialStatus>("cursor_credential_status"),
          invoke<AppSettings>("load_app_settings"),
          invoke<Project[]>("list_projects"),
        ]);
        if (cancelled) return;
        setCredential(savedCredential);
        setTheme(settings.theme);
        setProjects(savedProjects);
        const project = savedProjects.find((item) => item.id === settings.activeProjectId)
          ?? savedProjects.find((item) => item.path === settings.workspace)
          ?? savedProjects[0];
        if (!project) return;
        setActiveProject(project);
        const savedChats = await invoke<Chat[]>("list_chats", { projectId: project.id, archived: false });
        if (cancelled) return;
        setChats(savedChats);
        const chat = savedChats.find((item) => item.id === settings.activeChatId) ?? savedChats[0];
        if (chat) {
          setActiveChat(chat);
          setSession(await invoke<TaskSession | null>("load_chat_session", { chatId: chat.id }) ?? undefined);
        }
        setReport(await invoke<EnvironmentReport>("inspect_environment", { workspace: project.path }));
      } catch (reason) { if (!cancelled) setError(String(reason)); }
      finally { if (!cancelled) setBusy(false); }
    }
    void restore();
    return () => { cancelled = true; };
  }, []);

  const filteredChats = useMemo(() => chats.filter((chat) => chat.title.toLowerCase().includes(search.toLowerCase())), [chats, search]);
  const resumesAfterGrok = session?.task.status === "blocked" && ["validating", "reviewing"].includes(session.blockedFrom ?? "");
  const canExecute = report?.codex.state === "ready" && (resumesAfterGrok || report?.cursor.state === "ready");

  async function refreshChats(project = activeProject, archived = showArchived) {
    if (!project) return [];
    const items = await invoke<Chat[]>("list_chats", { projectId: project.id, archived });
    setChats(items);
    return items;
  }

  async function chooseProject(project: Project) {
    setProjectMenu(false);
    setActiveProject(project);
    setActiveChat(undefined);
    setSession(undefined);
    setGoal("");
    setShowArchived(false);
    await invoke("select_project", { projectId: project.id });
    const items = await invoke<Chat[]>("list_chats", { projectId: project.id, archived: false });
    setChats(items);
    setReport(await invoke<EnvironmentReport>("inspect_environment", { workspace: project.path }));
  }

  async function addExisting() {
    setProjectMenu(false);
    const path = await open({ directory: true, multiple: false, title: "Add Git repository" });
    if (!path) return;
    setBusy(true);
    try {
      const project = await invoke<Project>("add_existing_project", { path });
      const items = await invoke<Project[]>("list_projects");
      setProjects(items);
      await chooseProject(project);
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  }

  async function pickDestination() {
    const path = await open({ directory: true, multiple: false, title: "Select an empty folder" });
    if (path) setModalPath(path);
  }

  async function submitProjectModal() {
    if (!modal || modal === "settings" || !modalPath.trim()) return;
    setBusy(true);
    setError(undefined);
    try {
      const project = modal === "create"
        ? await invoke<Project>("create_repository_project", { path: modalPath })
        : await invoke<Project>("clone_repository_project", { url: cloneUrl, destination: modalPath });
      setProjects(await invoke<Project[]>("list_projects"));
      setModal(undefined);
      setModalPath("");
      setCloneUrl("");
      await chooseProject(project);
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  }

  async function newChat() {
    if (!activeProject) { setProjectMenu(true); return; }
    const chat = await invoke<Chat>("create_chat", { projectId: activeProject.id, title: "New chat" });
    setChats((current) => [chat, ...current]);
    setActiveChat(chat);
    setSession(undefined);
    setGoal("");
  }

  async function openChat(chat: Chat) {
    setActiveChat(chat);
    setGoal("");
    setSession(await invoke<TaskSession | null>("load_chat_session", { chatId: chat.id }) ?? undefined);
  }

  async function archiveCurrentChat() {
    if (!activeChat) return;
    await invoke("archive_chat", { chatId: activeChat.id, archived: !activeChat.archived });
    setActiveChat(undefined);
    setSession(undefined);
    await refreshChats();
  }

  async function createPlan() {
    if (!goal.trim() || !activeProject || workflowPending.current) return;
    workflowPending.current = true;
    setWorkflowBusy("planning");
    setError(undefined);
    try {
      const chat = !activeChat || activeChat.taskId
        ? await invoke<Chat>("create_chat", { projectId: activeProject.id, title: goal.slice(0, 64) })
        : activeChat;
      setActiveChat(chat);
      const planned = await invoke<TaskSession>("create_task_plan", {
        chatId: chat.id,
        request: {
          goal: goal.trim(), workspace: activeProject.path, constraints: nonEmptyLines(constraints),
          acceptanceCriteria: nonEmptyLines(acceptance), validationCommands: commandArgs(validationCommands),
        },
      });
      setSession(planned);
      setGoal("");
      const updatedChats = await refreshChats();
      setActiveChat(updatedChats.find((item) => item.id === chat.id) ?? chat);
    } catch (reason) { setError(String(reason)); }
    finally { workflowPending.current = false; setWorkflowBusy(undefined); }
  }

  async function executePlan() {
    if (!session || workflowPending.current) return;
    workflowPending.current = true;
    setWorkflowBusy("executing");
    setError(undefined);
    try { setSession(await invoke<TaskSession>("execute_task", { taskId: session.task.id })); await refreshChats(); }
    catch (reason) { setError(String(reason)); }
    finally { workflowPending.current = false; setWorkflowBusy(undefined); }
  }

  async function diagnose() {
    if (!activeProject) return;
    setBusy(true);
    try {
      const saved = await invoke<CredentialStatus>("set_cursor_api_key", { value: cursorKey });
      setCredential(saved); setCursorKey("");
      setReport(await invoke<EnvironmentReport>("inspect_environment", { workspace: activeProject.path }));
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  }

  async function login() {
    try { await invoke("start_codex_login"); } catch (reason) { setError(String(reason)); }
  }

  async function setAppTheme(value: Theme) {
    setTheme(value);
    try { await invoke("save_theme_setting", { theme: value }); } catch (reason) { setError(String(reason)); }
  }

  async function uiAction(action: () => Promise<unknown>) {
    try { await action(); } catch (reason) { setError(String(reason)); }
  }

  async function showDetails(panel: "changes" | "terminal") {
    setDetails(panel);
    if (panel !== "changes" || !session?.worktree) return;
    setDetailsBusy(true);
    setDiff("");
    try { setDiff(await invoke<string>("read_task_diff", { taskId: session.task.id })); }
    catch (reason) { setDiff(String(reason)); }
    finally { setDetailsBusy(false); }
  }

  async function toggleArchive() {
    const next = !showArchived;
    setShowArchived(next);
    await refreshChats(activeProject, next);
  }

  function openSettings(tab = "Agents") {
    setSettingsTab(tab);
    setModal("settings");
  }

  useEffect(() => {
    function shortcut(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setModal(undefined); setAppMenu(undefined); setProjectMenu(false);
        setHeaderMenu(false); setDetails(undefined); setSearchVisible(false);
      }
      if (!(event.ctrlKey || event.metaKey)) return;
      if (event.key.toLowerCase() === "l") { event.preventDefault(); void uiAction(newChat); }
      if (event.key.toLowerCase() === "b") { event.preventDefault(); setSidebarVisible((value) => !value); }
      if (event.key.toLowerCase() === "k") { event.preventDefault(); setSidebarVisible(true); setSearchVisible(true); }
      if (event.key === ",") { event.preventDefault(); openSettings(); }
    }
    document.addEventListener("keydown", shortcut);
    return () => document.removeEventListener("keydown", shortcut);
  }, [activeProject]);

  const chatIndex = chats.findIndex((chat) => chat.id === activeChat?.id);
  const windowAction = (action: "minimize" | "toggleMaximize" | "close") => uiAction(() => getCurrentWindow()[action]());

  const actionLabel = workflowBusy === "executing" ? "Working…" : resumesAfterGrok ? "Continue checks" : session?.task.status === "needsRevision" || session?.task.status === "blocked" ? "Run Grok again" : "Build";

  return <div className={"cursor-shell" + (sidebarVisible ? "" : " sidebar-hidden") + (toolsVisible ? "" : " tools-hidden")}>
    <header className="window-menu" onMouseDown={(event) => {
      if (event.button === 0 && (event.target as HTMLElement).closest("[data-window-drag]")) void uiAction(() => getCurrentWindow().startDragging());
    }}>
      <span className="app-emblem" title="CodexBridge">Y</span>
      <nav aria-label="Application menu">{(["File", "Edit", "View", "Help"] as const).map((name) =>
        <div className="menu-anchor" key={name}><button className={appMenu === name ? "selected" : ""} onClick={() => setAppMenu(appMenu === name ? undefined : name)}>{name}</button>
          {appMenu === name && <div className="menu-popover" role="menu">
            {name === "File" && <><button onClick={() => { setAppMenu(undefined); void uiAction(newChat); }}>New Chat <kbd>Ctrl L</kbd></button><button onClick={() => { setAppMenu(undefined); void addExisting(); }}>Open repository…</button><button onClick={() => { setAppMenu(undefined); setModal("create"); }}>Create repository…</button><button onClick={() => { setAppMenu(undefined); setModal("clone"); }}>Clone repository…</button></>}
            {name === "Edit" && <><button onClick={() => { setAppMenu(undefined); setSidebarVisible(true); setSearchVisible(true); }}>Search chats <kbd>Ctrl K</kbd></button><button onClick={() => { setAppMenu(undefined); openSettings(); }}>Settings <kbd>Ctrl ,</kbd></button></>}
            {name === "View" && <><button onClick={() => { setSidebarVisible(!sidebarVisible); setAppMenu(undefined); }}>Toggle sidebar <kbd>Ctrl B</kbd></button><button onClick={() => { setToolsVisible(!toolsVisible); setAppMenu(undefined); }}>Toggle tools</button><hr />{(["light", "dark", "system"] as const).map((value) => <button key={value} onClick={() => { void setAppTheme(value); setAppMenu(undefined); }}>{theme === value ? <Check size={13} /> : <span className="menu-spacer" />}{value[0].toUpperCase() + value.slice(1)} theme</button>)}</>}
            {name === "Help" && <div className="about-app"><strong>CodexBridge</strong><span>Astra plans · Grok builds</span><span>Ctrl L — new chat<br />Ctrl K — search<br />Ctrl B — sidebar<br />Enter — send · Shift Enter — new line</span></div>}
          </div>}
        </div>)}</nav>
      <div className="window-drag" data-window-drag onDoubleClick={() => void windowAction("toggleMaximize")} />
      <div className="window-controls"><button aria-label="Minimize" onClick={() => void windowAction("minimize")}><Minus size={13} /></button><button aria-label="Maximize or restore" onClick={() => void windowAction("toggleMaximize")}><Square size={11} /></button><button aria-label="Close window" onClick={() => void windowAction("close")}><X size={15} /></button></div>
    </header>
    {sidebarVisible && <aside className="task-sidebar">
      <div className="sidebar-controls"><button title="Hide sidebar (Ctrl B)" aria-label="Hide sidebar" onClick={() => setSidebarVisible(false)}><PanelLeft size={15} /></button><div><button aria-label="Previous chat" disabled={chatIndex < 0 || chatIndex >= chats.length - 1} onClick={() => void uiAction(() => openChat(chats[chatIndex + 1]))}><ArrowLeft size={15} /></button><button aria-label="Next chat" disabled={chatIndex <= 0} onClick={() => void uiAction(() => openChat(chats[chatIndex - 1]))}><ArrowRight size={15} /></button></div></div>
      <nav className="primary-navigation" aria-label="Main navigation">
        <button onClick={() => void uiAction(newChat)}><Send size={15} />New Chat</button>
        <button onClick={() => setSearchVisible(!searchVisible)}><Search size={15} />Search</button>
        <button disabled title="Automations are not available yet"><Bot size={15} />Automations</button>
        <button onClick={() => openSettings("Rules")}><Blocks size={15} />Customize</button>
      </nav>
      {searchVisible && <label className="task-search"><Search size={14} /><input autoFocus aria-label="Search chats" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search chats…" /><button aria-label="Close search" onClick={() => { setSearchVisible(false); setSearch(""); }}><X size={12} /></button></label>}
      <section className="projects-section">
        <div className="section-caption"><span>Projects</span><button title="Create repository" aria-label="Create repository" onClick={() => setModal("create")}><Plus size={15} /></button></div>
        <button className="new-project" onClick={() => setModal("create")}><span className="dotted-circle" />New Project</button>
      </section>
      <section className="repositories-section">
        <div className="section-caption"><span>{showArchived ? "Archived chats" : "Repositories"}</span><div><button className={showArchived ? "selected" : ""} title={showArchived ? "Show active chats" : "Show archived chats"} aria-label="Toggle archived chats" onClick={() => void uiAction(toggleArchive)}><ListFilter size={15} /></button><div className="menu-anchor"><button title="Add repository" aria-label="Add repository" onClick={() => setProjectMenu(!projectMenu)}><FolderGit2 size={15} /></button>
          {projectMenu && <div className="project-menu"><button onClick={addExisting}><FolderOpen size={14} />Open repository…</button><button onClick={() => { setProjectMenu(false); setModal("create"); }}><Plus size={14} />Create repository…</button><button onClick={() => { setProjectMenu(false); setModal("clone"); }}><Copy size={14} />Clone repository…</button></div>}
        </div></div></div>
        <div className="repository-list">
          {projects.map((project) => <section className="repository" key={project.id}>
            <button className="repository-row" title={project.path} onClick={() => { if (activeProject?.id !== project.id) void uiAction(() => chooseProject(project)); }}><FolderOpen size={15} /><span>{project.name}</span></button>
            {activeProject?.id === project.id && filteredChats.map((chat) => <button key={chat.id} className={"chat-row" + (activeChat?.id === chat.id ? " active" : "")} title={chat.title + " · " + (statusLabel[chat.taskStatus ?? ""] ?? "New chat")} onClick={() => void uiAction(() => openChat(chat))}><span className={"task-status " + (chat.taskStatus ?? "draft")} /><span>{chat.title}</span><time>{relativeTime(chat.updatedAt)}</time></button>)}
          </section>)}
          {search && !filteredChats.length && <p className="sidebar-empty">No matching chats</p>}
          {!projects.length && <button className="repository-row" onClick={addExisting}><FolderOpen size={15} />Open repository…</button>}
        </div>
      </section>
      <footer className="sidebar-bottom"><button className="profile-button" onClick={() => openSettings()}><span className="profile-avatar">Y</span><span>CodexBridge</span></button><button aria-label="Settings" title="Settings (Ctrl ,)" onClick={() => openSettings()}><Settings size={15} /></button></footer>
    </aside>}

    <main className="agent-view">
      <header className="agent-header">
        <div>{!sidebarVisible && <button aria-label="Show sidebar" onClick={() => setSidebarVisible(true)}><PanelLeft size={15} /></button>}<strong>{activeChat?.title ?? "New Chat"}</strong><Laptop size={13} /></div>
        <div className="header-actions"><div className="menu-anchor"><button aria-label="Chat actions" onClick={() => setHeaderMenu(!headerMenu)}><MoreHorizontal size={16} /></button>{headerMenu && <div className="menu-popover align-right"><button disabled={!activeChat} onClick={() => { setHeaderMenu(false); void uiAction(archiveCurrentChat); }}><Archive size={14} />{activeChat?.archived ? "Restore chat" : "Archive chat"}</button><button onClick={() => { setHeaderMenu(false); openSettings(); }}><Settings size={14} />Settings</button></div>}</div><button aria-label="Toggle tools" onClick={() => setToolsVisible(!toolsVisible)}><PanelRight size={15} /></button></div>
      </header>
      {toolsVisible && <aside className="workspace-tools" aria-label="Workspace tools"><p>On {activeProject?.name ?? "this computer"}</p><button disabled={!session?.worktree} onClick={() => void showDetails("changes")}><GitCompareArrows size={15} />Changes</button><button disabled title="Browser is not available yet"><Globe size={15} />Browser</button><button onClick={() => void showDetails("terminal")}><TerminalSquare size={15} />Terminal</button><button disabled={!session?.worktree} onClick={() => void uiAction(() => invoke("open_task_worktree", { taskId: session!.task.id }))}><File size={15} />Files</button></aside>}

      <section className="agent-scroll">
        {!session && <div className="empty-agent"><h1>{activeProject ? "New Chat" : "Open a repository to get started"}</h1><p>{activeProject ? activeProject.name : "Create a project or add an existing repository from the sidebar."}</p></div>}
        {session && <div className="transcript">
          <article className="prompt-message"><p>{session.task.spec.goal}</p></article>
          {session.plan && <article className="assistant-message">
            <div className="actor"><span className="actor-icon astra"><BrainCircuit size={14} /></span><strong>Astra</strong><small>Planned</small></div>
            <MessageBody>{session.plan.summary}</MessageBody>
            {session.plan.workItems.length > 0 && <div className="plan-panel"><header><ListChecks size={14} /><strong>Plan</strong><span>{session.plan.workItems.length} tasks</span></header>{session.plan.workItems.map((item) => <div className="plan-task" key={item.id}><span>{item.id}</span><div><strong>{item.objective}</strong><small>{item.allowedPaths.join(" · ") || "Repository"}</small></div></div>)}</div>}
            {session.plan.risks.length > 0 && <div className="review-note"><CircleAlert size={13} />{session.plan.risks.join(" · ")}</div>}
          </article>}
          {session.worker && <article className="assistant-message">
            <div className="actor"><span className="actor-icon grok"><Bot size={14} /></span><strong>Grok 4.6</strong><small>Worked for {(session.worker.durationMs / 60000).toFixed(1)}m</small></div>
            <MessageBody>{session.worker.text || "Work completed."}</MessageBody>
            {session.worktree && <button className="worktree-card" onClick={() => void uiAction(() => invoke("open_task_worktree", { taskId: session.task.id }))}><FolderOpen size={16} /><span><strong>Open worktree</strong><small>{session.worktree}</small></span></button>}
          </article>}
          {session.validation.length > 0 && <article className="assistant-message">
            <div className="actor"><span className="actor-icon runner"><TerminalSquare size={14} /></span><strong>Checks</strong><small>{session.validation.every((item) => item.success) ? "Passed" : "Failed"}</small></div>
            <div className="checks">{session.validation.map((item) => <div key={item.command.join(" ")} className={item.success ? "pass" : "fail"}>{item.success ? <CheckCircle2 size={14} /> : <CircleAlert size={14} />}<code>{item.command.join(" ")}</code><b>{item.success ? "PASS" : "FAIL"}</b></div>)}</div>
          </article>}
          {session.review && <article className="assistant-message">
            <div className="actor"><span className="actor-icon review"><ShieldCheck size={14} /></span><strong>Astra Review</strong><small>{session.review.approved ? "Approved" : "Changes requested"}</small></div>
            <MessageBody>{session.review.summary}</MessageBody>{session.review.issues.map((issue) => <div className="review-note error" key={issue}><CircleAlert size={13} />{issue}</div>)}
          </article>}
        </div>}
      </section>

      <div className="composer-region"><footer className="prompt-dock">
        {error && <div className="inline-error"><CircleAlert size={14} /><span>{error}</span><button onClick={() => setError(undefined)}><X size={13} /></button></div>}
        {session?.plan && ["planning", "needsRevision", "blocked"].includes(session.task.status) && <button className="build-button" onClick={executePlan} disabled={!canExecute || !!workflowBusy}>{workflowBusy === "executing" ? <LoaderCircle className="spin" size={14} /> : <Play size={13} fill="currentColor" />}{actionLabel}</button>}
        {workflowBusy && <div className="working-status" role="status"><LoaderCircle className="spin" size={13} />{workflowBusy === "planning" ? "Astra is preparing a plan…" : "Grok is working · checks and Astra review will follow"}</div>}
        <div className={"prompt-box" + (goal.includes("\n") ? " expanded" : "")}>
          <button className="context-button" aria-label="Task context and constraints" title="Task context and constraints" onClick={() => openSettings("Task defaults")}><Plus size={17} /></button>
          <textarea ref={composerInput} aria-label="Message" value={goal} onChange={(event) => setGoal(event.target.value)} rows={1} placeholder={session ? "Send follow-up" : "Plan, ask, build anything"} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void createPlan(); } }} />
          <button className="model-selector" title="Current planner: Astra · High. Worker: Grok 4.6 · Extra High Fast" onClick={() => openSettings("Models")}>Astra · High<ChevronDown size={11} /></button>
          <button className="send" aria-label="Send message" onClick={createPlan} disabled={!goal.trim() || !activeProject || !!workflowBusy}>{workflowBusy === "planning" ? <LoaderCircle className="spin" size={14} /> : <ArrowUp size={16} />}</button>
        </div>
        <div className="composer-meta"><span title={session?.task.spec.baseCommit}><GitBranch size={13} />{session ? session.task.spec.baseCommit.slice(0, 8) : "Repository"}<ChevronDown size={11} /></span><span title={activeProject?.path}><Laptop size={13} />This PC<ChevronDown size={11} /></span><button aria-label="Connection status" title={"Astra: " + (report?.codex.title ?? "Not checked") + " · Grok: " + (report?.cursor.title ?? "Not checked")} onClick={() => openSettings()}><CircleDot size={13} /></button></div>
      </footer></div>
    </main>
    {details && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) setDetails(undefined); }}><section className="details-panel"><header><strong>{details === "changes" ? "Changes" : "Terminal output"}</strong><button aria-label="Close details" onClick={() => setDetails(undefined)}><X size={16} /></button></header><div className="details-content">{details === "changes" ? <pre>{detailsBusy ? "Loading changes…" : diff || "No tracked changes."}</pre> : session?.validation.length ? session.validation.map((check, index) => <section key={index}><h3><span className={check.success ? "pass" : "fail"}>{check.success ? "Passed" : "Failed"}</span> {check.command.join(" ")}</h3><pre>{check.output || "No output."}</pre></section>) : <p>No validation commands have run in this chat.</p>}</div></section></div>}


    {modal && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) setModal(undefined); }}><div className={`modal ${modal === "settings" ? "settings-modal" : ""}`}>
      <header><div>{modal === "settings" && <Settings size={16} />}<strong>{modal === "create" ? "Create repository" : modal === "clone" ? "Clone repository" : "Settings"}</strong></div><button onClick={() => setModal(undefined)}><X size={16} /></button></header>
      {modal === "settings" ? <div className="settings-body">
        <nav>{["Agents", "Models", "Rules", "Skills", "Task defaults", "Appearance"].map((tab) => <button key={tab} className={settingsTab === tab ? "active" : ""} onClick={() => setSettingsTab(tab)}>{tab}</button>)}</nav>
        <div className="settings-content">{error && <div className="inline-error" role="alert">{error}</div>}<div hidden={!["Agents", "Models"].includes(settingsTab)}><h2>{settingsTab}</h2><p>{settingsTab === "Models" ? "Current configuration: Astra · High and Grok 4.6 · Extra High Fast. Model and effort selection will be connected in M4." : "Connections and defaults for the planner and worker."}</p>
          <div className="setting-card"><div><BrainCircuit size={16} /><span><strong>GPT-6 Astra</strong><small>{report?.codex.title ?? "Not checked"}</small></span><StatusDot state={report?.codex.state} /></div>{report?.codex.state === "needsLogin" && <button onClick={login}>Sign in with ChatGPT</button>}</div>
          <div className="setting-card"><div><Bot size={16} /><span><strong>Grok 4.6 xHigh Fast</strong><small>{credential?.configured ? `Key saved in ${credential.backend}` : "Cursor API key required"}</small></span><StatusDot state={report?.cursor.state} /></div><label><KeyRound size={14} /><input type="password" value={cursorKey} onChange={(event) => setCursorKey(event.target.value)} placeholder={credential?.masked ?? "crsr_…"} /></label></div>
          <button className="settings-action" onClick={diagnose} disabled={busy || !activeProject}>{busy ? <LoaderCircle className="spin" size={14} /> : <CircleDot size={14} />}Refresh connections</button>
          </div><div hidden={settingsTab !== "Task defaults"}><h2>Task defaults</h2>
          <label className="setting-field"><span>Constraints</span><textarea rows={3} value={constraints} onChange={(event) => setConstraints(event.target.value)} /></label>
          <label className="setting-field"><span>Acceptance criteria</span><textarea rows={3} value={acceptance} onChange={(event) => setAcceptance(event.target.value)} /></label>
          <label className="setting-field"><span>Validation commands</span><textarea rows={3} value={validationCommands} onChange={(event) => setValidationCommands(event.target.value)} /></label>
          </div><div hidden={settingsTab !== "Appearance"}><h2>Appearance</h2><div className="theme-options"><button className={theme === "system" ? "active" : ""} onClick={() => setAppTheme("system")}><Clock3 size={14} />System</button><button className={theme === "light" ? "active" : ""} onClick={() => setAppTheme("light")}><Sun size={14} />Light</button><button className={theme === "dark" ? "active" : ""} onClick={() => setAppTheme("dark")}><Moon size={14} />Dark</button></div>
          </div>{["Rules", "Skills"].includes(settingsTab) && <div><h2>{settingsTab}</h2><p className="settings-explanation">{settingsTab === "Rules" ? "The project Rules editor is not connected yet. Use Task defaults to specify constraints for the next task." : "The Skills library is not connected yet."}</p>{settingsTab === "Rules" && <button className="settings-action" onClick={() => setSettingsTab("Task defaults")}>Open task defaults</button>}</div>}
        </div>
      </div> : <div className="project-form">
        {modal === "clone" && <label><span>Repository URL</span><input value={cloneUrl} onChange={(event) => setCloneUrl(event.target.value)} placeholder="https://github.com/org/repo.git" /></label>}
        <label><span>{modal === "clone" ? "Destination" : "Repository folder"}</span><div><input value={modalPath} onChange={(event) => setModalPath(event.target.value)} placeholder="D:\\projects\\new-project" /><button onClick={pickDestination}><FolderOpen size={15} /></button></div></label>
        {error && <div className="form-error">{error}</div>}
        <footer><button onClick={() => setModal(undefined)}>Cancel</button><button className="primary" onClick={submitProjectModal} disabled={busy || !modalPath.trim() || (modal === "clone" && !cloneUrl.trim())}>{busy && <LoaderCircle className="spin" size={14} />}{modal === "clone" ? "Clone" : "Create"}</button></footer>
      </div>}
    </div></div>}
  </div>;
}
