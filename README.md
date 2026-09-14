# Yarocursor

Yarocursor is a desktop agent client for an opinionated two-agent workflow:

- GPT-6 Astra plans work and reviews the result through Codex App Server.
- Cursor Grok 4.6 executes coding tasks through Cursor SDK Bridge.
- The Rust orchestrator owns state, Git isolation, validation, retries, and budgets.

The current build contains the first end-to-end local orchestration loop and the new agent-client shell. The UI restores the active repository and theme, verifies both providers in the background, accepts a task in chat, asks Astra for a structured read-only plan, waits for explicit user approval, runs Grok 4.6 xHigh Fast in an isolated Git worktree, executes validation commands, and sends the resulting diff to Astra for a read-only verdict. Task state, artifacts, and non-secret settings are persisted in SQLite under the application data directory.

## Prerequisites

- Rust 1.85 or newer
- Node.js 22.13 or newer
- Codex CLI with `codex app-server`
- Git
- Windows x64 or Linux x64/arm64
- A Cursor user API key

Install the pinned Cursor SDK Bridge release:

```powershell
.\scripts\install-cursor-bridge.ps1
```

```sh
./scripts/install-cursor-bridge.sh
```

Authenticate Codex with the ChatGPT subscription:

```text
codex login
```

## Run

```text
npm install
npm run tauri dev
```

The Cursor key entered in the UI is stored by the operating system (Windows Credential Manager or Linux Secret Service) and is restored when Yarocursor starts. It is never written to SQLite or frontend storage. You can remove it from the settings panel. You can also run the terminal diagnostic with `CURSOR_API_KEY` already set:

```text
cargo run -p yarocursor-core --bin yarocursor-doctor -- /path/to/project
```

The Astra planning path has a small CLI smoke test:

```text
cargo run -p yarocursor-core --bin yarocursor-plan -- /path/to/project "Describe the requested change"
```

## Current security boundary

Astra runs with Codex App Server's `readOnly` sandbox and never receives write access. Grok runs inside a detached Git worktree created from the recorded base commit. Cursor's local SDK sandbox is disabled because it is unavailable in some supported desktop environments, including Windows; the worktree is the current worker isolation boundary. Cursor agent state uses the bridge's JSONL store in a writable per-task application-data directory, avoiding reliance on Cursor's default SQLite path. Validation commands are launched directly from argument arrays without a shell. The original checkout is not used as the worker directory; completed worktrees remain available for inspection and manual integration.
