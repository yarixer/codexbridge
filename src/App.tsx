import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  Bot,
  BrainCircuit,
  Check,
  ChevronRight,
  CircleAlert,
  FolderGit2,
  KeyRound,
  LoaderCircle,
  Play,
  Settings2,
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
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
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
        <div className="version">v0.1</div>
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
