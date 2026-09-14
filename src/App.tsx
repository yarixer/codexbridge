import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  Archive, ArrowLeft, Bot, BrainCircuit, Check, CheckCircle2, ChevronDown,
  CircleAlert, CircleDot, Clock3, Copy, FolderGit2, FolderOpen, GitBranch,
  KeyRound, ListChecks, LoaderCircle, MessageSquarePlus, Moon, MoreHorizontal,
  PanelLeftClose, Play, Plus, Search, Send, Settings, ShieldCheck, Sparkles,
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

function taskGroup(chat: Chat) {
  if (["completed", "needsRevision", "blocked"].includes(chat.taskStatus ?? "")) return "review";
  if (chat.taskStatus) return "progress";
  return "recent";
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
  const [projectMenu, setProjectMenu] = useState(false);
  const [modal, setModal] = useState<"create" | "clone" | "settings">();
  const [modalPath, setModalPath] = useState("");
  const [cloneUrl, setCloneUrl] = useState("");
  const [showArchived, setShowArchived] = useState(false);
  const [busy, setBusy] = useState(false);
  const [workflowBusy, setWorkflowBusy] = useState<"planning" | "executing">();
  const [error, setError] = useState<string>();
  const workflowPending = useRef(false);

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
  const groups = useMemo(() => ({
    progress: filteredChats.filter((chat) => taskGroup(chat) === "progress"),
    review: filteredChats.filter((chat) => taskGroup(chat) === "review"),
    recent: filteredChats.filter((chat) => taskGroup(chat) === "recent"),
  }), [filteredChats]);
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
    const path = await open({ directory: true, multiple: false, title: "Добавить Git-репозиторий" });
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
    const path = await open({ directory: true, multiple: false, title: "Выберите пустую папку" });
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
    const chat = await invoke<Chat>("create_chat", { projectId: activeProject.id, title: "Новый чат" });
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

  function renderGroup(title: string, items: Chat[]) {
    if (!items.length) return null;
    return <section className="task-group"><h3>{title}<span>{items.length}</span></h3>{items.map((chat) =>
      <button key={chat.id} className={`task-item ${activeChat?.id === chat.id ? "active" : ""}`} onClick={() => openChat(chat)}>
        <span className={`task-status ${chat.taskStatus ?? "draft"}`} />
        <span><strong>{chat.title}</strong><small>{statusLabel[chat.taskStatus ?? ""] ?? (chat.taskId ? "Task" : "Draft")}</small></span>
        <MoreHorizontal size={13} />
      </button>)}</section>;
  }

  const actionLabel = workflowBusy === "executing" ? "Working…" : resumesAfterGrok ? "Continue checks" : session?.task.status === "needsRevision" || session?.task.status === "blocked" ? "Run Grok again" : "Build";

  return <div className="cursor-shell">
    <aside className="task-sidebar">
      <header className="app-title"><div className="cursor-logo">Y</div><strong>Yarocursor</strong><button><PanelLeftClose size={15} /></button></header>
      <div className="project-select-wrap">
        <button className="project-select" onClick={() => setProjectMenu(!projectMenu)}><FolderGit2 size={15} /><span><strong>{activeProject?.name ?? "Open project"}</strong><small>{activeProject?.path ?? "Add a repository"}</small></span><ChevronDown size={13} /></button>
        {projectMenu && <div className="project-menu">
          {projects.map((project) => <button key={project.id} onClick={() => chooseProject(project)}><FolderGit2 size={14} /><span><strong>{project.name}</strong><small>{project.path}</small></span>{activeProject?.id === project.id && <Check size={13} />}</button>)}
          {projects.length > 0 && <hr />}
          <button onClick={addExisting}><FolderOpen size={14} /> Add existing repository</button>
          <button onClick={() => { setProjectMenu(false); setModal("create"); }}><Plus size={14} /> Create repository</button>
          <button onClick={() => { setProjectMenu(false); setModal("clone"); }}><Copy size={14} /> Clone repository</button>
        </div>}
      </div>
      <button className="new-agent" onClick={newChat}><MessageSquarePlus size={15} /> New agent <kbd>Ctrl L</kbd></button>
      <label className="task-search"><Search size={14} /><input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search agents" /></label>
      <div className="task-list">
        {renderGroup("In Progress", groups.progress)}
        {renderGroup("Ready for Review", groups.review)}
        {renderGroup(showArchived ? "Archived" : "Recent", groups.recent)}
        {!filteredChats.length && <div className="empty-tasks"><Sparkles size={18} /><span>No agent tasks yet</span><small>Start a new agent to build something.</small></div>}
      </div>
      <footer className="sidebar-bottom">
        <button onClick={async () => { const next = !showArchived; setShowArchived(next); setChats(activeProject ? await invoke("list_chats", { projectId: activeProject.id, archived: next }) : []); }}><Archive size={14} /> {showArchived ? "Back to agents" : "Archive"}</button>
        <button onClick={() => setModal("settings")}><Settings size={14} /> Settings</button>
        <span><StatusDot state={report?.codex.state} /><StatusDot state={report?.cursor.state} /></span>
      </footer>
    </aside>

    <main className="agent-view">
      <header className="agent-header">
        <div><strong>{activeChat?.title ?? "New agent"}</strong><span>{activeProject ? <><FolderGit2 size={11} />{activeProject.name}</> : "No project selected"}{session && <><GitBranch size={11} />{session.task.spec.baseCommit.slice(0, 8)}</>}</span></div>
        <div>{activeChat && <button title="Archive" onClick={archiveCurrentChat}><Archive size={15} /></button>}<button><MoreHorizontal size={16} /></button></div>
      </header>

      <section className="agent-scroll">
        {!session && <div className="empty-agent"><div className="empty-orb"><Sparkles size={24} /></div><h1>Build something</h1><p>Describe what you want to create or change. Astra will plan it, then Grok will implement it in an isolated worktree.</p></div>}
        {session && <div className="transcript">
          <article className="prompt-message"><p>{session.task.spec.goal}</p></article>
          {session.plan && <article className="assistant-message">
            <div className="actor"><span className="actor-icon astra"><BrainCircuit size={14} /></span><strong>Astra</strong><small>Planned</small></div>
            <p>{session.plan.summary}</p>
            <div className="plan-panel"><header><ListChecks size={14} /><strong>Plan</strong><span>{session.plan.workItems.length} tasks</span></header>{session.plan.workItems.map((item) => <div className="plan-task" key={item.id}><span>{item.id}</span><div><strong>{item.objective}</strong><small>{item.allowedPaths.join(" · ") || "Repository"}</small></div></div>)}</div>
            {session.plan.risks.length > 0 && <div className="review-note"><CircleAlert size={13} />{session.plan.risks.join(" · ")}</div>}
          </article>}
          {session.worker && <article className="assistant-message">
            <div className="actor"><span className="actor-icon grok"><Bot size={14} /></span><strong>Grok 4.6</strong><small>Worked for {(session.worker.durationMs / 60000).toFixed(1)}m</small></div>
            <p className="pre-wrap">{session.worker.text || "Work completed."}</p>
            {session.worktree && <button className="worktree-card" onClick={() => invoke("open_task_worktree", { taskId: session.task.id })}><FolderOpen size={16} /><span><strong>Open worktree</strong><small>{session.worktree}</small></span></button>}
          </article>}
          {session.validation.length > 0 && <article className="assistant-message">
            <div className="actor"><span className="actor-icon runner"><TerminalSquare size={14} /></span><strong>Checks</strong><small>{session.validation.every((item) => item.success) ? "Passed" : "Failed"}</small></div>
            <div className="checks">{session.validation.map((item) => <div key={item.command.join(" ")} className={item.success ? "pass" : "fail"}>{item.success ? <CheckCircle2 size={14} /> : <CircleAlert size={14} />}<code>{item.command.join(" ")}</code><b>{item.success ? "PASS" : "FAIL"}</b></div>)}</div>
          </article>}
          {session.review && <article className="assistant-message">
            <div className="actor"><span className="actor-icon review"><ShieldCheck size={14} /></span><strong>Astra Review</strong><small>{session.review.approved ? "Approved" : "Changes requested"}</small></div>
            <p>{session.review.summary}</p>{session.review.issues.map((issue) => <div className="review-note error" key={issue}><CircleAlert size={13} />{issue}</div>)}
          </article>}
        </div>}
      </section>

      <footer className="prompt-dock">
        {error && <div className="inline-error"><CircleAlert size={14} /><span>{error}</span><button onClick={() => setError(undefined)}><X size={13} /></button></div>}
        {session?.plan && ["planning", "needsRevision", "blocked"].includes(session.task.status) && <button className="build-button" onClick={executePlan} disabled={!canExecute || !!workflowBusy}>{workflowBusy === "executing" ? <LoaderCircle className="spin" size={14} /> : <Play size={13} fill="currentColor" />}{actionLabel}</button>}
        <div className="prompt-box"><textarea value={goal} onChange={(event) => setGoal(event.target.value)} rows={3} placeholder="Plan, search, build anything" onKeyDown={(event) => { if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) void createPlan(); }} /><div className="prompt-toolbar"><div><button><Plus size={14} /> Add context</button><button><Sparkles size={13} /> Plan <ChevronDown size={11} /></button></div><div><button>Astra · High <ChevronDown size={11} /></button><button className="send" onClick={createPlan} disabled={!goal.trim() || !activeProject || !!workflowBusy}>{workflowBusy === "planning" ? <LoaderCircle className="spin" size={14} /> : <Send size={14} />}</button></div></div></div>
        <small>/ for commands · @ for files · Ctrl+Enter to send</small>
      </footer>
    </main>

    {modal && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) setModal(undefined); }}><div className={`modal ${modal === "settings" ? "settings-modal" : ""}`}>
      <header><div>{modal === "settings" && <Settings size={16} />}<strong>{modal === "create" ? "Create repository" : modal === "clone" ? "Clone repository" : "Settings"}</strong></div><button onClick={() => setModal(undefined)}><X size={16} /></button></header>
      {modal === "settings" ? <div className="settings-body">
        <nav><button className="active">Agents</button><button>Models</button><button>Rules</button><button>Skills</button><button>Appearance</button></nav>
        <div className="settings-content"><h2>Agents</h2><p>Connections and defaults for the planner and worker.</p>
          <div className="setting-card"><div><BrainCircuit size={16} /><span><strong>GPT-6 Astra</strong><small>{report?.codex.title ?? "Not checked"}</small></span><StatusDot state={report?.codex.state} /></div>{report?.codex.state === "needsLogin" && <button onClick={login}>Sign in with ChatGPT</button>}</div>
          <div className="setting-card"><div><Bot size={16} /><span><strong>Grok 4.6 xHigh Fast</strong><small>{credential?.configured ? `Key saved in ${credential.backend}` : "Cursor API key required"}</small></span><StatusDot state={report?.cursor.state} /></div><label><KeyRound size={14} /><input type="password" value={cursorKey} onChange={(event) => setCursorKey(event.target.value)} placeholder={credential?.masked ?? "crsr_…"} /></label></div>
          <button className="settings-action" onClick={diagnose} disabled={busy || !activeProject}>{busy ? <LoaderCircle className="spin" size={14} /> : <CircleDot size={14} />}Refresh connections</button>
          <h2>Task defaults</h2>
          <label className="setting-field"><span>Constraints</span><textarea rows={3} value={constraints} onChange={(event) => setConstraints(event.target.value)} /></label>
          <label className="setting-field"><span>Acceptance criteria</span><textarea rows={3} value={acceptance} onChange={(event) => setAcceptance(event.target.value)} /></label>
          <label className="setting-field"><span>Validation commands</span><textarea rows={3} value={validationCommands} onChange={(event) => setValidationCommands(event.target.value)} /></label>
          <h2>Appearance</h2><div className="theme-options"><button className={theme === "system" ? "active" : ""} onClick={() => setAppTheme("system")}><Clock3 size={14} />System</button><button className={theme === "light" ? "active" : ""} onClick={() => setAppTheme("light")}><Sun size={14} />Light</button><button className={theme === "dark" ? "active" : ""} onClick={() => setAppTheme("dark")}><Moon size={14} />Dark</button></div>
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
