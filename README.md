# Yarocursor

Yarocursor is a small desktop control plane for an opinionated two-agent workflow:

- GPT-6 Astra plans work and reviews the result through Codex App Server.
- Cursor Grok 4.6 executes coding tasks through Cursor SDK Bridge.
- The Rust orchestrator owns state, Git isolation, validation, retries, and budgets.

The project is at milestone 0: provider discovery and account diagnostics. The UI already verifies the local Codex session, Astra availability, Cursor Bridge connectivity, the Cursor model catalog, and ChatGPT rate-limit data.

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

The Cursor key entered in the milestone-0 UI lives only in process memory and is cleared when the app exits. You can also run the terminal diagnostic with `CURSOR_API_KEY` already set:

```text
cargo run -p yarocursor-core --bin yarocursor-doctor -- /path/to/project
```

## Current security boundary

The Astra path is configured as a read-only role in the workflow design. The first milestone only performs account and model discovery; it does not start a model turn or modify project files. Worker sandboxing and worktree enforcement arrive with the execution loop before Grok is allowed to edit a project.

