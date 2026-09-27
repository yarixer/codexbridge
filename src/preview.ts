// Local, opt-in UI fixtures. Vite removes this module from production builds.
// No credentials, files, model requests, or user database are used here.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

export function installPreview() {
  const params = new URLSearchParams(location.search);
  const long = params.get("preview") === "workflow";
  const project = { id: "preview-project", name: "codexbridge", path: "/preview/codexbridge", createdAt: Date.now(), updatedAt: Date.now() };
  let chats = [{ id: "preview-chat", projectId: project.id, title: long ? "Visualizing a quadratic function" : "Current time inquiry", archived: false, createdAt: Date.now() - 60000, updatedAt: Date.now() - 60000, taskId: "preview-task", taskStatus: long ? "needsRevision" : "completed" }];
  const health = { state: "ready", title: "Preview fixture", detail: "Local preview only", models: [], usage: null };
  const session = {
    task: { id: "preview-task", status: long ? "needsRevision" : "completed", workerAttempts: 1, spec: { goal: long ? "I need to understand how to write a Python program that visualizes y=x squared. Give me a very brief plan." : "what time is it?", workspace: project.path, baseCommit: "0de100c4", maxWorkerAttempts: 3 } },
    plan: {
      summary: long ? "Add a self-contained Python example: calculate values of **y = x²** and plot the graph with matplotlib." : "It's **9:39 AM** on Monday, September 14, 2026 (UTC+2).",
      workItems: long ? [{ id: "plot-square", objective: "Plot the parabola, label the axes, and add a grid.", allowedPaths: ["examples/plot_square.py", "README.md"], dependsOn: [], acceptanceCriteria: ["Vertex at (0, 0); symmetry about the y-axis."] }] : [],
      risks: [],
    },
    validation: long ? [
      { command: ["cargo", "test", "--workspace"], success: true, output: "Preview validation output: 11 passed", exitCode: 0 },
      { command: ["npm", "run", "build"], success: true, output: "Preview validation output: build completed", exitCode: 0 },
    ] : [],
    ...(long ? {
      worktree: "/preview/worktrees/plot-square",
      worker: { status: "completed", durationMs: 91000, events: [], text: [
        "The example was added to `examples/plot_square.py`.",
        "",
        "### Running",
        "",
        "```bash",
        "python -m pip install matplotlib",
        "python examples/plot_square.py",
        "```",
        "",
        "Checks:",
        "- Vertex: **(0, 0)**.",
        "- Opposite x values produce the same y values.",
        "- Axes are labeled and the grid is enabled.",
        "",
        "### Without a graphical window",
        "",
        "```python",
        "import matplotlib.pyplot as plt",
        "xs = [i / 10 for i in range(-100, 101)]",
        "plt.plot(xs, [x ** 2 for x in xs])",
        "plt.savefig('parabola.png')",
        "```",
        "",
        "Details were saved in the README."
      ].join("\n") },
      review: { approved: false, summary: "The code and README follow the plan. The graph requires visual verification.", issues: ["Confirm the vertex at (0, 0) and the parabola's symmetry."] },
    } : {}),
  };
  mockWindows("main");
  mockIPC((command, raw) => {
    const args = raw as Record<string, unknown> | undefined;
    switch (command) {
      case "cursor_credential_status": return { configured: true, backend: "Preview", masked: "Preview only" };
      case "load_app_settings": return { workspace: project.path, theme: params.get("theme") ?? "light", activeProjectId: project.id, activeChatId: chats[0]?.id };
      case "list_projects": return [project];
      case "list_chats": return chats.filter((chat) => chat.archived === args?.archived);
      case "load_chat_session": return args?.chatId === "preview-chat" ? session : null;
      case "inspect_environment": return { codex: health, cursor: health, workspace: project.path };
      case "create_chat": {
        const chat = { ...chats[0], id: "draft-" + Date.now(), title: "New Chat", taskId: "", taskStatus: "", archived: false, updatedAt: Date.now() };
        chats = [chat, ...chats];
        return chat;
      }
      case "archive_chat": chats = chats.map((chat) => chat.id === args?.chatId ? { ...chat, archived: Boolean(args?.archived) } : chat); return;
      case "save_theme_setting": case "select_project": return;
      case "read_task_diff": return "diff --git a/examples/plot_square.py b/examples/plot_square.py\n+plt.plot(xs, [x ** 2 for x in xs])";
      default: throw new Error("Unavailable in UI preview: " + command);
    }
  });
}
