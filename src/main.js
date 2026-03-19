import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import katexCssUri from "katex/dist/katex.min.css?url";
import texmathCssUri from "markdown-it-texmath/css/texmath.css?url";
import {
  TODO_TASK_PATTERN,
  DEFAULT_THEME_PREFERENCE,
  createMarkdownRenderer,
  formatDisplayFileName,
  normalizeThemePreference,
  parseMarkdownTodos,
  buildSectionListMarkdown,
  getWebviewHtml
} from "./todomark-core";

const UI_STATE_KEY = "todomark.uiState";
const THEME_STATE_KEY = "todomark.themePreference";
const MERMAID_SCRIPT_URI = "/mermaid.min.js";

const markdownRenderer = createMarkdownRenderer();

let activeFilePath = null;
let markdownContent = "";
let markdownMtimeMs = null;
let writeInFlight = false;
let pollTimer = null;

const readJsonStorage = (key, fallback) => {
  try {
    const raw = localStorage.getItem(key);
    if (!raw) {
      return fallback;
    }
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? parsed : fallback;
  } catch {
    return fallback;
  }
};

const writeJsonStorage = (key, value) => {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Ignore persistence failures (private mode, quota limits, etc.)
  }
};

const setNativeWindowTitle = async (title) => {
  const safeTitle =
    typeof title === "string" && title.trim().length > 0 ? title : "TodoMark";
  document.title = safeTitle;

  try {
    await getCurrentWindow().setTitle(safeTitle);
  } catch {
    // Ignore title sync failures.
  }
};

const isMarkdownPath = (pathValue) =>
  typeof pathValue === "string" && pathValue.toLowerCase().endsWith(".md");

const getQueryFilePath = () => {
  const query = new URLSearchParams(window.location.search);
  const file = query.get("file");
  return file && file.trim().length > 0 ? file : null;
};

const getLaunchFilePath = () => {
  const launchFile = window.__TODOMARK_LAUNCH_FILE__;
  return typeof launchFile === "string" && launchFile.trim().length > 0
    ? launchFile
    : null;
};

const setQueryFilePath = (pathValue) => {
  const query = new URLSearchParams(window.location.search);
  if (pathValue) {
    query.set("file", pathValue);
  } else {
    query.delete("file");
  }
  const search = query.toString();
  const nextUrl = `${window.location.pathname}${search ? `?${search}` : ""}`;
  history.replaceState({}, "", nextUrl);
};

const pickMarkdownFile = async () => {
  const picked = await openDialog({
    directory: false,
    multiple: false,
    filters: [
      {
        name: "Markdown",
        extensions: ["md"]
      }
    ]
  });

  return typeof picked === "string" ? picked : null;
};

const applyLineReplacements = (content, replacementsByLine) => {
  const eol = content.includes("\r\n") ? "\r\n" : "\n";
  const lines = content.split(/\r?\n/);

  for (const [lineNumber, marker] of replacementsByLine) {
    const line = lines[lineNumber];
    if (typeof line !== "string") {
      throw new Error("Task line out of range.");
    }

    const match = line.match(TODO_TASK_PATTERN);
    if (!match) {
      throw new Error("Task line no longer matches markdown todo format.");
    }

    lines[lineNumber] = `${match[1]}${marker}${match[3]}${match[4]}`;
  }

  return lines.join(eol);
};

const computeTaskToggle = (content, lineNumber) => {
  const tasks = parseMarkdownTodos(content, markdownRenderer);
  const taskByLine = new Map(tasks.map((task) => [task.lineNumber, task]));
  const targetTask = taskByLine.get(lineNumber);
  if (!targetTask) {
    return { updatedContent: content, updates: [] };
  }

  /** @type {Map<number, boolean>} */
  const desiredStateByLine = new Map();
  const nextCompletedState = !targetTask.completed;
  const hasDescendants =
    Array.isArray(targetTask.descendantLineNumbers) &&
    targetTask.descendantLineNumbers.length > 0;

  desiredStateByLine.set(targetTask.lineNumber, nextCompletedState);
  if (hasDescendants) {
    targetTask.descendantLineNumbers.forEach((descendantLineNumber) => {
      desiredStateByLine.set(descendantLineNumber, nextCompletedState);
    });
  }

  if (targetTask.completed) {
    let ancestorLineNumber = targetTask.parentLineNumber;
    while (Number.isInteger(ancestorLineNumber)) {
      const ancestorTask = taskByLine.get(ancestorLineNumber);
      if (!ancestorTask) {
        break;
      }
      if (ancestorTask.completed) {
        desiredStateByLine.set(ancestorTask.lineNumber, false);
      }
      ancestorLineNumber = ancestorTask.parentLineNumber;
    }
  } else {
    desiredStateByLine.set(targetTask.lineNumber, true);
  }

  const lines = content.split(/\r?\n/);
  /** @type {Map<number, " " | "x">} */
  const replacementsByLine = new Map();
  /** @type {{ lineNumber: number; completed: boolean }[]} */
  const updates = [];

  for (const [affectedLineNumber, desiredCompletedState] of desiredStateByLine) {
    if (
      !Number.isInteger(affectedLineNumber) ||
      affectedLineNumber < 0 ||
      affectedLineNumber >= lines.length
    ) {
      continue;
    }

    const line = lines[affectedLineNumber];
    const match = line.match(TODO_TASK_PATTERN);
    if (!match) {
      continue;
    }

    const currentCompletedState = match[2].toLowerCase() === "x";
    if (currentCompletedState === desiredCompletedState) {
      continue;
    }

    const marker = desiredCompletedState ? "x" : " ";
    replacementsByLine.set(affectedLineNumber, marker);
    updates.push({
      lineNumber: affectedLineNumber,
      completed: desiredCompletedState
    });
  }

  if (replacementsByLine.size === 0) {
    return { updatedContent: content, updates: [] };
  }

  const updatedContent = applyLineReplacements(content, replacementsByLine);
  return { updatedContent, updates };
};

const renderEmptyState = (message) => {
  setQueryFilePath(null);
  void setNativeWindowTitle("TodoMark");
  document.body.innerHTML = `
    <main style="
      min-height: 100vh;
      margin: 0;
      display: grid;
      place-items: center;
      background: linear-gradient(135deg, #fff6fb, #edf5ff 52%, #f1fff7);
      color: #27344d;
      font-family: 'Avenir Next', 'Segoe UI', sans-serif;
      padding: 24px;
      box-sizing: border-box;
    ">
      <section style="
        width: min(520px, 100%);
        border-radius: 16px;
        border: 1px solid rgba(88, 103, 130, 0.24);
        background: rgba(255, 255, 255, 0.88);
        backdrop-filter: blur(6px);
        box-shadow: 0 20px 40px rgba(72, 88, 120, 0.2);
        padding: 24px;
      ">
        <h1 style="margin: 0 0 10px; font-size: 1.35rem;">TodoMark</h1>
        <p style="margin: 0 0 18px; color: #55657f; line-height: 1.5;">${message}</p>
        <button
          id="pick-markdown-file"
          style="
            appearance: none;
            border: 1px solid rgba(88, 103, 130, 0.24);
            border-radius: 999px;
            padding: 10px 16px;
            background: #de79a9;
            color: #fff;
            cursor: pointer;
            font-weight: 600;
          "
        >
          Open Markdown File
        </button>
      </section>
    </main>
  `;

  const pickButton = document.getElementById("pick-markdown-file");
  pickButton?.addEventListener("click", async () => {
    const picked = await pickMarkdownFile();
    if (!picked) {
      return;
    }

    await loadAndRenderMarkdown(picked);
  });
};

const dispatchTaskUpdates = (updates) => {
  window.postMessage(
    {
      type: "tasksToggled",
      updates
    },
    "*"
  );
};

const syncMarkdownFromDisk = async (pathValue) => {
  const response = await invoke("read_markdown", { path: pathValue });
  markdownContent = response.content;
  markdownMtimeMs = Number(response.mtimeMs);
  activeFilePath = pathValue;
};

const loadAndRenderMarkdown = async (pathValue) => {
  if (!isMarkdownPath(pathValue)) {
    renderEmptyState("Please choose a valid .md file.");
    return;
  }

  await syncMarkdownFromDisk(pathValue);

  const tasks = parseMarkdownTodos(markdownContent, markdownRenderer);
  const markdownHtml = markdownRenderer.render(markdownContent);
  const sectionListMarkdown = buildSectionListMarkdown(markdownContent);
  const sectionListHtml = markdownRenderer.render(sectionListMarkdown);
  const initialThemePreference = normalizeThemePreference(
    readJsonStorage(THEME_STATE_KEY, DEFAULT_THEME_PREFERENCE)
  );
  const displayName = formatDisplayFileName(pathValue);

  setQueryFilePath(pathValue);
  void setNativeWindowTitle(displayName);

  const html = getWebviewHtml({
    filePath: pathValue,
    tasks,
    markdownHtml,
    sectionListHtml,
    katexCssUri,
    texmathCssUri,
    mermaidScriptUri: MERMAID_SCRIPT_URI,
    initialThemePreference
  });

  document.open();
  document.write(html);
  document.close();
};

window.__TODOMARK_BRIDGE__ = {
  getState() {
    return readJsonStorage(UI_STATE_KEY, {});
  },

  setState(nextState) {
    const safeState = nextState && typeof nextState === "object" ? nextState : {};
    writeJsonStorage(UI_STATE_KEY, safeState);
  },

  async postMessage(message) {
    if (!message || typeof message.type !== "string") {
      return;
    }

    if (message.type === "saveThemePreference") {
      writeJsonStorage(
        THEME_STATE_KEY,
        normalizeThemePreference(message.preference)
      );
      return;
    }

    if (message.type !== "toggleTask") {
      return;
    }

    if (!activeFilePath) {
      return;
    }

    const lineNumber = Number(message.lineNumber);
    if (!Number.isInteger(lineNumber) || lineNumber < 0) {
      return;
    }

    try {
      const { updatedContent, updates } = computeTaskToggle(markdownContent, lineNumber);
      if (updates.length === 0 || updatedContent === markdownContent) {
        return;
      }

      writeInFlight = true;
      const response = await invoke("write_markdown", {
        path: activeFilePath,
        content: updatedContent
      });

      markdownContent = updatedContent;
      markdownMtimeMs = Number(response.mtimeMs);
      dispatchTaskUpdates(updates);
    } catch (error) {
      console.error("Failed to toggle task", error);
    } finally {
      writeInFlight = false;
    }
  }
};

const startPolling = () => {
  if (pollTimer) {
    clearInterval(pollTimer);
  }

  pollTimer = setInterval(async () => {
    if (!activeFilePath || writeInFlight) {
      return;
    }

    try {
      const response = await invoke("stat_markdown", { path: activeFilePath });
      const diskMtime = Number(response.mtimeMs);
      if (!Number.isFinite(diskMtime) || !Number.isFinite(markdownMtimeMs)) {
        return;
      }

      if (diskMtime === markdownMtimeMs) {
        return;
      }

      await loadAndRenderMarkdown(activeFilePath);
    } catch (error) {
      console.error("Failed to stat markdown file", error);
    }
  }, 1500);
};

const boot = async () => {
  startPolling();

  const initialPath = getQueryFilePath() || getLaunchFilePath();
  if (initialPath) {
    try {
      await loadAndRenderMarkdown(initialPath);
      return;
    } catch (error) {
      console.error("Failed to load markdown from startup argument", error);
    }
  }

  try {
    const picked = await pickMarkdownFile();
    if (picked) {
      await loadAndRenderMarkdown(picked);
      return;
    }
  } catch (error) {
    console.error("Failed to open file picker", error);
  }

  renderEmptyState("No markdown file selected. Open a .md file to begin.");
};

void boot();
