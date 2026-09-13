import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  Bot,
  BrainCircuit,
  Check,
  ChevronRight,
  CircleAlert,
  FolderGit2,
  GitBranch,
  Hammer,
  KeyRound,
  ListChecks,
  LoaderCircle,
  Play,
  Settings2,
  ShieldCheck,
  TerminalSquare,
} from "lucide-react";

type HealthState = "ready" | "needsLogin" | "needsApiKey" | "missingDependency" | "unavailable";

type Model = {
  id: string;
  displayName: string;
  parameters: unknown;
};

type ProviderHealth = {
  state: HealthState;
  title: string;
  detail: string;
  models: Model[];
  usage: unknown | null;
};

type EnvironmentReport = {
  codex: ProviderHealth;
  cursor: ProviderHealth;
  workspace: string;
};

type WorkItem = {
  id: string;
  objective: string;
  allowedPaths: string[];
  dependsOn: string[];
  acceptanceCriteria: string[];
};

type TaskSession = {
  task: {
    id: string;
    status: string;
    workerAttempts: number;
    spec: { baseCommit: string; maxWorkerAttempts: number };
  };
  plan?: { summary: string; workItems: WorkItem[]; risks: string[] };
  worktree?: string;
  worker?: { status: string; text: string; durationMs: number; events: { kind: string; summary: string }[] };
  validation: { command: string[]; success: boolean; exitCode?: number; output: string }[];
  review?: { approved: boolean; summary: string; issues: string[] };
};

const initialWorkspace = "D:\\programming\\yarocodex";

function StatusDot({ state }: { state: HealthState }) {
  return <span className={`status-dot ${state}`} aria-label={state} />;
}

function ProviderCard({ icon, eyebrow, health }: { icon: React.ReactNode; eyebrow: string; health?: ProviderHealth }) {
  return (
    <article className="provider-card">
      <div className="provider-icon">{icon}</div>
      <div className="provider-copy">
        <span className="eyebrow">{eyebrow}</span>
        <h3>{health?.title ?? "Не проверено"}</h3>
        <p>{health?.detail ?? "Запусти диагностику подключения."}</p>
      </div>
      <StatusDot state={health?.state ?? "unavailable"} />
    </article>
  );
}

export default function App() {
  const [workspace, setWorkspace] = useState(initialWorkspace);
  const [cursorKey, setCursorKey] = useState("");
  const [report, setReport] = useState<EnvironmentReport>();
  const [busy, setBusy] = useState(false);
  const [loginStarted, setLoginStarted] = useState(false);
  const [error, setError] = useState<string>();
  const [goal, setGoal] = useState("");
  const [constraints, setConstraints] = useState("");
  const [acceptance, setAcceptance] = useState("");
  const [validationCommands, setValidationCommands] = useState("cargo test --workspace\nnpm run build");
  const [session, setSession] = useState<TaskSession>();
  const [workflowBusy, setWorkflowBusy] = useState<"planning" | "executing">();
  const workflowPending = useRef(false);

  useEffect(() => {
    invoke<TaskSession | null>("latest_task_session").then((latest) => {
      if (latest) setSession(latest);
    }).catch(() => undefined);
  }, []);

  const readyCount = useMemo(
    () => [report?.codex.state, report?.cursor.state].filter((state) => state === "ready").length,
    [report],
  );

  async function diagnose() {
    setBusy(true);
    setError(undefined);
    try {
      await invoke("set_cursor_api_key", { value: cursorKey });
      setReport(await invoke<EnvironmentReport>("inspect_environment", { workspace }));
      setLoginStarted(false);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  function nonEmptyLines(value: string) {
    return value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  }

  function commandArgs(value: string) {
    return nonEmptyLines(value).map((line) =>
      Array.from(line.matchAll(/"([^"]*)"|'([^']*)'|([^\s]+)/g), (match) => match[1] ?? match[2] ?? match[3]),
    );
  }

  async function createPlan() {
    if (workflowPending.current) return;
    workflowPending.current = true;
    setError(undefined);
    setWorkflowBusy("planning");
    setSession(undefined);
    try {
      const request = {
        goal: goal.trim(),
        workspace,
        constraints: nonEmptyLines(constraints),
        acceptanceCriteria: nonEmptyLines(acceptance),
        validationCommands: commandArgs(validationCommands),
      };
      setSession(await invoke<TaskSession>("create_task_plan", { request }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      workflowPending.current = false;
      setWorkflowBusy(undefined);
    }
  }

  async function executePlan() {
    if (!session || workflowPending.current) return;
    workflowPending.current = true;
    setError(undefined);
    setWorkflowBusy("executing");
    try {
      setSession(await invoke<TaskSession>("execute_task", { taskId: session.task.id }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      workflowPending.current = false;
      setWorkflowBusy(undefined);
    }
  }

  async function login() {
    setError(undefined);
    setLoginStarted(false);
    try {
      await invoke("start_codex_login");
      setLoginStarted(true);
    } catch (reason) {
      setError(String(reason));
    }
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand-mark"><span>Y</span></div>
        <nav>
          <button className="nav-button active" title="Среда"><Activity size={20} /></button>
          <button className="nav-button" title="Проекты"><FolderGit2 size={20} /></button>
          <button className="nav-button" title="Запуски"><Play size={20} /></button>
          <button className="nav-button" title="Настройки"><Settings2 size={20} /></button>
        </nav>
        <div className="version">v0.2</div>
      </aside>

      <main>
        <header className="topbar">
          <div>
            <span className="eyebrow">CONTROL PLANE</span>
            <h1>Подключение агентов</h1>
          </div>
          <div className="readiness">
            <span>{readyCount}/2 готовы</span>
            <div className="readiness-track"><i style={{ width: `${readyCount * 50}%` }} /></div>
          </div>
        </header>

        <section className="content-grid">
          <div className="primary-column">
            <div className="intro">
              <div className="kicker"><TerminalSquare size={15} /> Этап 0 · проверка интеграций</div>
              <h2>Сначала проверим канал управления.</h2>
              <p>Astra будет планировать и проверять изменения. Grok получит рабочую копию проекта и выполнит реализацию.</p>
            </div>

            <div className="providers">
              <ProviderCard icon={<BrainCircuit size={24} />} eyebrow="ARCHITECT · CODEX" health={report?.codex} />
              <ProviderCard icon={<Bot size={24} />} eyebrow="WORKER · CURSOR" health={report?.cursor} />
            </div>

            {report?.codex.state === "needsLogin" && (
              <button className="inline-action" onClick={login}><KeyRound size={17} /> Войти через ChatGPT <ChevronRight size={16} /></button>
            )}

            {loginStarted && (
              <div className="login-banner"><Check size={18} /><span>Браузер открыт. Заверши вход в ChatGPT, затем снова запусти диагностику.</span></div>
            )}

            {error && <div className="error-banner"><CircleAlert size={18} /><span>{error}</span></div>}

            <button className="primary-action" onClick={diagnose} disabled={busy || !workspace.trim()}>
              {busy ? <LoaderCircle className="spin" size={18} /> : <Activity size={18} />}
              {busy ? "Проверяю…" : "Запустить диагностику"}
            </button>

            {readyCount === 2 && (
              <section className="workflow">
                <div className="section-heading">
                  <span className="step-number">01</span>
                  <div><span className="eyebrow">ORCHESTRATION</span><h3>Новая задача</h3></div>
                </div>

                <label>
                  <span>Цель</span>
                  <textarea rows={4} placeholder="Что нужно реализовать или исправить?" value={goal} onChange={(event) => setGoal(event.target.value)} />
                </label>
                <div className="two-columns">
                  <label>
                    <span>Ограничения · по одному на строку</span>
                    <textarea rows={4} placeholder="Не менять публичный API" value={constraints} onChange={(event) => setConstraints(event.target.value)} />
                  </label>
                  <label>
                    <span>Критерии приёмки · по одному на строку</span>
                    <textarea rows={4} placeholder="Все тесты проходят" value={acceptance} onChange={(event) => setAcceptance(event.target.value)} />
                  </label>
                </div>
                <label>
                  <span>Команды проверки · по одной на строку</span>
                  <textarea className="mono" rows={3} value={validationCommands} onChange={(event) => setValidationCommands(event.target.value)} />
                </label>

                <button className="secondary-action" onClick={createPlan} disabled={!goal.trim() || !!workflowBusy}>
                  {workflowBusy === "planning" ? <LoaderCircle className="spin" size={18} /> : <BrainCircuit size={18} />}
                  {workflowBusy === "planning" ? "Astra изучает проект…" : "Составить план с Astra"}
                </button>

                {session?.plan && (
                  <div className="plan-card">
                    <div className="result-heading"><BrainCircuit size={19} /><div><span className="eyebrow">ASTRA PLAN</span><strong>{session.plan.summary}</strong></div></div>
                    <div className="commit-line"><GitBranch size={14} /> base {session.task.spec.baseCommit.slice(0, 10)}</div>
                    <ol className="work-items">
                      {session.plan.workItems.map((item) => (
                        <li key={item.id}><span>{item.id}</span><div><strong>{item.objective}</strong><small>{item.allowedPaths.join(" · ") || "весь worktree"}</small></div></li>
                      ))}
                    </ol>
                    {session.plan.risks.length > 0 && <p className="risk-line">Риски: {session.plan.risks.join(" · ")}</p>}
                    {(session.task.status === "planning" || session.task.status === "needsRevision" || session.task.status === "blocked") && session.task.workerAttempts < session.task.spec.maxWorkerAttempts && (
                      <button className="primary-action" onClick={executePlan} disabled={!!workflowBusy}>
                        {workflowBusy === "executing" ? <LoaderCircle className="spin" size={18} /> : <Hammer size={18} />}
                        {workflowBusy === "executing" ? "Grok работает, затем Astra проверит…" : session.task.status === "needsRevision" || session.task.status === "blocked" ? "Повторить запуск Grok" : "Утвердить план и запустить Grok"}
                      </button>
                    )}
                  </div>
                )}

                {session?.worker && (
                  <div className="result-card">
                    <div className="result-heading"><Hammer size={19} /><div><span className="eyebrow">GROK RESULT</span><strong>{session.worker.status}</strong></div></div>
                    <p>{session.worker.text || "Исполнитель завершил работу без итогового сообщения."}</p>
                    {session.worktree && <code>{session.worktree}</code>}
                  </div>
                )}

                {session && session.validation.length > 0 && (
                  <div className="result-card">
                    <div className="result-heading"><ListChecks size={19} /><div><span className="eyebrow">VALIDATION</span><strong>{session.validation.every((item) => item.success) ? "Все проверки прошли" : "Нужны исправления"}</strong></div></div>
                    {session.validation.map((item) => <div className={`check-row ${item.success ? "pass" : "fail"}`} key={item.command.join(" ")}><span>{item.success ? "PASS" : "FAIL"}</span><code>{item.command.join(" ")}</code></div>)}
                  </div>
                )}

                {session?.review && (
                  <div className="result-card">
                    <div className="result-heading"><ShieldCheck size={19} /><div><span className="eyebrow">ASTRA REVIEW</span><strong>{session.review.approved ? "Одобрено" : "Нужна доработка"}</strong></div></div>
                    <p>{session.review.summary}</p>
                    {session.review.issues.map((issue) => <div className="review-issue" key={issue}>{issue}</div>)}
                  </div>
                )}
              </section>
            )}
          </div>

          <aside className="settings-panel">
            <div className="panel-heading">
              <Settings2 size={18} />
              <div><span className="eyebrow">SESSION</span><h3>Параметры запуска</h3></div>
            </div>

            <label>
              <span>Рабочая папка</span>
              <div className="input-wrap"><FolderGit2 size={17} /><input value={workspace} onChange={(event) => setWorkspace(event.target.value)} /></div>
            </label>

            <label>
              <span>Cursor API key</span>
              <div className="input-wrap"><KeyRound size={17} /><input type="password" placeholder="crsr_…" value={cursorKey} onChange={(event) => setCursorKey(event.target.value)} /></div>
              <small>Хранится только в памяти до закрытия приложения.</small>
            </label>

            <div className="policy-card">
              <span className="policy-icon"><Check size={16} /></span>
              <div><strong>Astra: только чтение</strong><p>План и ревью без изменения файлов.</p></div>
            </div>

            <div className="model-spec">
              <div><span>Brain</span><strong>gpt-6-astra</strong></div>
              <div><span>Worker</span><strong>grok-4.6 · xhigh · fast</strong></div>
            </div>
          </aside>
        </section>
      </main>
    </div>
  );
}
