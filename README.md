# CodexBridge

CodexBridge is a Tauri desktop client where Astra plans and reviews coding tasks while Grok implements them in isolated Git worktrees.

## Run

Requirements: Rust 1.85+, Node.js 22.13+, Git, Codex CLI, and a Cursor API key.

```powershell
.\scripts\install-cursor-bridge.ps1
codex login
npm install
npm run tauri dev
```

Cursor API keys are stored in the operating system credential store, not in SQLite or frontend storage.
