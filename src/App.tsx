import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity, Bot, BrainCircuit, Check, CheckCircle2, ChevronDown, CircleAlert, Clock3,
  Code2, FileCode2, FolderGit2, FolderOpen, GitBranch, KeyRound, ListChecks,
  LoaderCircle, Menu, MessageSquarePlus, Moon, MoreHorizontal, PanelRight, Play,
  Plus, Search, Send, Settings, ShieldCheck, Sparkles, Sun, TerminalSquare,
} from "lucide-react";

type HealthState = "ready" | "needsLogin" | "needsApiKey" | "missingDependency" | "unavailable";
type Theme = "system" | "light" | "dark";
type Model = { id: string; displayName: string; parameters: unknown };
type ProviderHealth = { state: HealthState; title: string; detail: string; models: Model[]; usage: unknown | null };
type EnvironmentReport = { codex: ProviderHealth; cursor: ProviderHealth; workspace: string };
type CredentialStatus = { configured: boolean; masked?: string; backend: string; error?: string };
type AppSettings = { workspace: string; theme: Theme };
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
  draft: "Черновик", planning: "Astra планирует", executing: "Grok выполняет",
  validating: "Проверка", reviewing: "Ревью Astra", needsRevision: "Нужна доработка",
  blocked: "Приостановлено", completed: "Завершено",
};

function StatusDot({ state }: { state?: HealthState }) {
  return <span className={`status-dot ${state ?? "unavailable"}`} />;
}

function compactPath(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) || "Выберите репозиторий";
}

function nonEmptyLines(value: string) {
  return value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
}

function commandArgs(value: string) {
  return nonEmptyLines(value).map((line) =>
    Array.from(line.matchAll(/"([^"]*)"|'([^']*)'|([^\s]+)/g), (match) => match[1] ?? match[2] ?? match[3]),
  );
}

export default function App() {
  const [workspace, setWorkspace] = useState("");
  const [theme, setTheme] = useState<Theme>("system");
  const [cursorKey, setCursorKey] = useState("");
  const [credential, setCredential] = useState<CredentialStatus>();
  const [report, setReport] = useState<EnvironmentReport>();
  const [session, setSession] = useState<TaskSession>();
  const [goal, setGoal] = useState("");
  const [constraints, setConstraints] = useState("");
  const [acceptance, setAcceptance] = useState("");
  const [validationCommands, setValidationCommands] = useState("cargo test --workspace\nnpm run build");
  const [busy, setBusy] = useState(false);
  const [workflowBusy, setWorkflowBusy] = useState<"planning" | "executing">();
  const [loginStarted, setLoginStarted] = useState(false);
  const [error, setError] = useState<string>();
  const [inspectorOpen, setInspectorOpen] = useState(true);
  const workflowPending = useRef(false);

  useEffect(() => {
    let cancelled = false;
    async function restore() {
      setBusy(true);
      try {
        const [latest, savedCredential, settings] = await Promise.all([
          invoke<TaskSession | null>("latest_task_session"),
          invoke<CredentialStatus>("cursor_credential_status"),
          invoke<AppSettings>("load_app_settings"),
        ]);
        if (cancelled) return;
        if (latest) setSession(latest);
        setCredential(savedCredential);
        setWorkspace(settings.workspace);
        setTheme(settings.theme);
        if (settings.workspace) {
          const environment = await invoke<EnvironmentReport>("inspect_environment", { workspace: settings.workspace });
          if (!cancelled) setReport(environment);
        }
      } catch (reason) {
        if (!cancelled) setError(String(reason));
      } finally {
        if (!cancelled) setBusy(false);
      }
    }
    void restore();
    return () => { cancelled = true; };
  }, []);

  useEffect(() => { document.documentElement.dataset.theme = theme; }, [theme]);

  const readyCount = useMemo(
    () => [report?.codex.state, report?.cursor.state].filter((state) => state === "ready").length,
    [report],
  );
  const resumesAfterGrok = session?.task.status === "blocked"
    && (session.blockedFrom === "validating" || session.blockedFrom === "reviewing");
  const canExecuteSession = report?.codex.state === "ready"
    && (resumesAfterGrok || report?.cursor.state === "ready");

  async function diagnose() {
    if (!workspace.trim()) return;
    setBusy(true);
    setError(undefined);
    try {
      const saved = await invoke<CredentialStatus>("set_cursor_api_key", { value: cursorKey });
      setCredential(saved);
      if (cursorKey.trim()) setCursorKey("");
      await invoke("save_workspace_setting", { workspace });
      setReport(await invoke<EnvironmentReport>("inspect_environment", { workspace }));
      setLoginStarted(false);
    } catch (reason) { setError(String(reason)); } finally { setBusy(false); }
  }

  async function selectTheme(value: Theme) {
    setTheme(value);
    try { await invoke("save_theme_setting", { theme: value }); } catch (reason) { setError(String(reason)); }
  }

  async function createPlan() {
    if (!goal.trim() || !workspace.trim() || workflowPending.current) return;
    workflowPending.current = true;
    setError(undefined);
    setWorkflowBusy("planning");
    setSession(undefined);
    try {
      await invoke("save_workspace_setting", { workspace });
      setSession(await invoke<TaskSession>("create_task_plan", {
        request: {
          goal: goal.trim(), workspace, constraints: nonEmptyLines(constraints),
          acceptanceCriteria: nonEmptyLines(acceptance), validationCommands: commandArgs(validationCommands),
        },
      }));
    } catch (reason) { setError(String(reason)); } finally {
      workflowPending.current = false;
      setWorkflowBusy(undefined);
    }
  }

  async function executePlan() {
    if (!session || workflowPending.current) return;
    workflowPending.current = true;
    setError(undefined);
    setWorkflowBusy("executing");
    try { setSession(await invoke<TaskSession>("execute_task", { taskId: session.task.id })); }
    catch (reason) { setError(String(reason)); }
    finally { workflowPending.current = false; setWorkflowBusy(undefined); }
  }

  async function openWorktree() {
    if (!session) return;
    try { await invoke("open_task_worktree", { taskId: session.task.id }); }
    catch (reason) { setError(String(reason)); }
  }

  async function login() {
    setError(undefined);
    try { await invoke("start_codex_login"); setLoginStarted(true); }
    catch (reason) { setError(String(reason)); }
  }

  async function deleteCursorKey() {
    try { setCredential(await invoke<CredentialStatus>("delete_cursor_api_key")); setCursorKey(""); setReport(undefined); }
    catch (reason) { setError(String(reason)); }
  }

  const actionLabel = workflowBusy === "executing" ? "Выполнение…" : resumesAfterGrok
    ? "Продолжить проверку" : session?.task.status === "needsRevision" || session?.task.status === "blocked"
      ? "Повторить запуск Grok" : "Утвердить и запустить";

  return (
    <div className={`app-shell ${inspectorOpen ? "inspector-open" : ""}`}>
      <aside className="activity-rail">
        <button className="logo-button" title="Yarocursor">Y</button>
        <div className="rail-group">
          <button className="rail-button active" title="Агенты"><Sparkles size={18} /></button>
          <button className="rail-button" title="Поиск"><Search size={18} /></button>
          <button className="rail-button" title="Rules"><FileCode2 size={18} /></button>
          <button className="rail-button" title="Skills"><Code2 size={18} /></button>
        </div>
        <button className="rail-button rail-bottom" title="Настройки"><Settings size={18} /></button>
      </aside>

      <aside className="project-sidebar">
        <div className="window-drag"><Menu size={15} /><span>Yarocursor</span></div>
        <button className="project-picker">
          <span className="project-icon"><FolderGit2 size={15} /></span>
          <span><strong>{compactPath(workspace)}</strong><small>{workspace || "Добавьте локальный репозиторий"}</small></span>
          <ChevronDown size={14} />
        </button>
        <button className="new-chat" onClick={() => { setSession(undefined); setGoal(""); }}>
          <MessageSquarePlus size={15} /> Новый чат <span>Ctrl L</span>
        </button>
        <div className="sidebar-section-title"><span>Чаты</span><MoreHorizontal size={14} /></div>
        <div className="chat-list">
          {session ? <button className="chat-item active">
            <span className={`chat-state ${session.task.status}`} />
            <span><strong>{session.task.spec.goal || session.plan?.summary || "Задача агента"}</strong><small>{statusLabel[session.task.status] ?? session.task.status}</small></span>
          </button> : <div className="empty-list">Здесь появятся чаты проекта</div>}
        </div>
        <div className="sidebar-footer">
          <div className="connection-summary"><span><StatusDot state={report?.codex.state} /> Astra</span><span><StatusDot state={report?.cursor.state} /> Grok</span></div>
          <button onClick={() => setInspectorOpen(true)}><Settings size={14} /> Настройки</button>
        </div>
      </aside>

      <main className="chat-pane">
        <header className="chat-header">
          <div><strong>{session?.task.spec.goal || "Новый чат"}</strong><span><GitBranch size={12} /> {session ? session.task.spec.baseCommit.slice(0, 8) : compactPath(workspace)}</span></div>
          <div className="header-actions">
            <span className="provider-pill"><StatusDot state={report?.codex.state} /> Astra + Grok</span>
            <button className={inspectorOpen ? "selected" : ""} onClick={() => setInspectorOpen(!inspectorOpen)} title="Инспектор"><PanelRight size={17} /></button>
          </div>
        </header>

        <section className={`conversation ${session ? "has-session" : ""}`}>
          {!session && <div className="welcome">
            <div className="welcome-mark"><Sparkles size={24} /></div>
            <h1>Что будем делать?</h1>
            <p>Astra изучит задачу и подготовит план. После утверждения Grok выполнит изменения в отдельной рабочей копии.</p>
            <div className="welcome-status"><span><StatusDot state={report?.codex.state} /> {report?.codex.title ?? "Проверяю Astra"}</span><span><StatusDot state={report?.cursor.state} /> {report?.cursor.title ?? "Проверяю Grok"}</span></div>
          </div>}

          {session && <div className="message-stream">
            <article className="user-message"><p>{session.task.spec.goal}</p></article>
            {session.plan && <article className="agent-message">
              <div className="agent-avatar astra"><BrainCircuit size={15} /></div>
              <div className="message-body">
                <div className="message-author"><strong>Astra</strong><span>Архитектор · high</span></div>
                <p>{session.plan.summary}</p>
                <div className="plan-block">
                  <div className="block-title"><ListChecks size={14} /> План реализации <span>{session.plan.workItems.length}</span></div>
                  {session.plan.workItems.map((item) => <div className="plan-row" key={item.id}><span>{item.id}</span><div><strong>{item.objective}</strong><small>{item.allowedPaths.join(" · ") || "Репозиторий"}</small></div></div>)}
                </div>
                {session.plan.risks.length > 0 && <div className="note-row"><CircleAlert size={14} /> {session.plan.risks.join(" · ")}</div>}
              </div>
            </article>}

            {session.worker && <article className="agent-message">
              <div className="agent-avatar grok"><Bot size={15} /></div>
              <div className="message-body">
                <div className="message-author"><strong>Grok 4.6</strong><span>xHigh · Fast · {(session.worker.durationMs / 1000).toFixed(1)}s</span></div>
                <p className="pre-wrap">{session.worker.text || "Исполнитель завершил работу без итогового сообщения."}</p>
                {session.worktree && <button className="artifact-card" onClick={openWorktree}><FolderOpen size={16} /><span><strong>Рабочая копия</strong><small>{session.worktree}</small></span><ChevronDown size={14} /></button>}
              </div>
            </article>}

            {session.validation.length > 0 && <article className="agent-message compact-message">
              <div className="agent-avatar runner"><TerminalSquare size={15} /></div>
              <div className="message-body">
                <div className="message-author"><strong>Локальная проверка</strong><span>{session.validation.every((item) => item.success) ? "Все команды прошли" : "Есть ошибки"}</span></div>
                <div className="validation-block">{session.validation.map((item) => <div className={item.success ? "pass" : "fail"} key={item.command.join(" ")}><span>{item.success ? <Check size={13} /> : <CircleAlert size={13} />}</span><code>{item.command.join(" ")}</code><b>{item.success ? "PASS" : "FAIL"}</b></div>)}</div>
              </div>
            </article>}

            {session.review && <article className="agent-message">
              <div className="agent-avatar review"><ShieldCheck size={15} /></div>
              <div className="message-body">
                <div className="message-author"><strong>Astra Review</strong><span>{session.review.approved ? "Принято" : "Требуется доработка"}</span></div>
                <p>{session.review.summary}</p>
                {session.review.issues.map((issue) => <div className="review-issue" key={issue}><CircleAlert size={13} />{issue}</div>)}
              </div>
            </article>}
            {session.worker && !session.review && <div className="pending-review"><LoaderCircle className={workflowBusy ? "spin" : ""} size={15} /> Astra review появится после локальной проверки</div>}
          </div>}
        </section>

        <footer className="composer-wrap">
          {error && <div className="error-banner"><CircleAlert size={15} /><span>{error}</span></div>}
          {session?.plan && ["planning", "needsRevision", "blocked"].includes(session.task.status) && <button className="run-plan-button" onClick={executePlan} disabled={!!workflowBusy || !canExecuteSession}>{workflowBusy === "executing" ? <LoaderCircle className="spin" size={15} /> : <Play size={15} fill="currentColor" />}{actionLabel}</button>}
          <div className="composer">
            <textarea rows={3} value={goal} onChange={(event) => setGoal(event.target.value)} placeholder="Планируй, ищи и создавай что угодно" onKeyDown={(event) => { if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) void createPlan(); }} />
            <div className="composer-toolbar">
              <div><button><Plus size={15} /> Контекст</button><button className="mode-button"><Sparkles size={14} /> План <ChevronDown size={12} /></button></div>
              <div><button className="model-button">Astra <ChevronDown size={12} /></button><button className="send-button" onClick={createPlan} disabled={!goal.trim() || !workspace.trim() || !!workflowBusy}>{workflowBusy === "planning" ? <LoaderCircle className="spin" size={15} /> : <Send size={15} />}</button></div>
            </div>
          </div>
          <small className="composer-hint">Ctrl+Enter — отправить · Astra не изменяет файлы</small>
        </footer>
      </main>

      {inspectorOpen && <aside className="inspector">
        <header><div><Activity size={15} /><strong>Состояние</strong></div><button onClick={() => setInspectorOpen(false)}>×</button></header>
        <div className="inspector-scroll">
          <section className="run-card">
            <div className="run-title"><span className={`run-dot ${workflowBusy ? "pulse" : ""}`} /><div><strong>{workflowBusy ? "Выполняется" : session ? statusLabel[session.task.status] ?? session.task.status : "Готов к задаче"}</strong><small>{session ? `Попытка ${session.task.workerAttempts}/${session.task.spec.maxWorkerAttempts}` : `${readyCount}/2 провайдера готовы`}</small></div></div>
            {session && <div className="stage-track"><i className="done" /><i className={session.worker ? "done" : ""} /><i className={session.validation.length ? "done" : ""} /><i className={session.review ? "done" : ""} /></div>}
          </section>

          <section className="inspector-section">
            <div className="section-label">Подключения</div>
            <div className="provider-line"><span><BrainCircuit size={15} /><span><strong>Astra</strong><small>{report?.codex.title ?? "Не проверено"}</small></span></span><StatusDot state={report?.codex.state} /></div>
            <div className="provider-line"><span><Bot size={15} /><span><strong>Grok 4.6</strong><small>{report?.cursor.title ?? "Не проверено"}</small></span></span><StatusDot state={report?.cursor.state} /></div>
            {report?.codex.state === "needsLogin" && <button className="text-action" onClick={login}><KeyRound size={14} /> Войти через ChatGPT</button>}
            {loginStarted && <div className="muted-note">Завершите вход в браузере, затем обновите подключения.</div>}
          </section>

          <section className="inspector-section">
            <div className="section-label">Репозиторий</div>
            <label className="field"><span>Локальная папка</span><div><FolderGit2 size={14} /><input value={workspace} onChange={(event) => setWorkspace(event.target.value)} placeholder="D:\\projects\\app" /></div></label>
          </section>

          <section className="inspector-section">
            <div className="section-label">Cursor</div>
            <label className="field"><span>API key</span><div><KeyRound size={14} /><input type="password" value={cursorKey} onChange={(event) => setCursorKey(event.target.value)} placeholder={credential?.masked ?? "crsr_…"} /></div></label>
            <div className="credential-state">{credential?.configured ? <><CheckCircle2 size={13} /> Сохранён в {credential.backend}</> : credential?.error ?? "Ключ будет сохранён системно"}</div>
            {credential?.configured && <button className="delete-key" onClick={deleteCursorKey}>Удалить ключ</button>}
            <button className="diagnose-button" onClick={diagnose} disabled={busy || !workspace.trim()}>{busy ? <LoaderCircle className="spin" size={14} /> : <Activity size={14} />} {busy ? "Проверяю…" : "Обновить подключения"}</button>
          </section>

          <section className="inspector-section">
            <div className="section-label">Параметры задачи</div>
            <label className="field textarea-field"><span>Ограничения</span><textarea rows={3} value={constraints} onChange={(event) => setConstraints(event.target.value)} placeholder="По одному на строку" /></label>
            <label className="field textarea-field"><span>Критерии приёмки</span><textarea rows={3} value={acceptance} onChange={(event) => setAcceptance(event.target.value)} placeholder="По одному на строку" /></label>
            <label className="field textarea-field"><span>Команды проверки</span><textarea rows={3} value={validationCommands} onChange={(event) => setValidationCommands(event.target.value)} /></label>
          </section>

          <section className="inspector-section">
            <div className="section-label">Оформление</div>
            <div className="theme-picker">
              <button className={theme === "system" ? "active" : ""} onClick={() => selectTheme("system")}><Clock3 size={14} /> Система</button>
              <button className={theme === "light" ? "active" : ""} onClick={() => selectTheme("light")}><Sun size={14} /> Светлая</button>
              <button className={theme === "dark" ? "active" : ""} onClick={() => selectTheme("dark")}><Moon size={14} /> Тёмная</button>
            </div>
          </section>
        </div>
      </aside>}
    </div>
  );
}
