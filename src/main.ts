import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AppState, LayoutInfo } from "./types";

const DEFAULT_HOTKEY = "CmdOrCtrl+Shift+Space";

const el = <T extends HTMLElement>(selector: string): T => {
  const node = document.querySelector<T>(selector);
  if (!node) throw new Error(`Missing element: ${selector}`);
  return node;
};

const banner = el<HTMLDivElement>("#banner");
const currentLayoutEl = el<HTMLSpanElement>("#current-layout");
const cycleBtn = el<HTMLButtonElement>("#cycle-btn");
const layoutList = el<HTMLUListElement>("#layout-list");
const layoutEmpty = el<HTMLParagraphElement>("#layout-empty");
const hotkeyBtn = el<HTMLButtonElement>("#hotkey-btn");
const hotkeyResetBtn = el<HTMLButtonElement>("#hotkey-reset");
const switchOnClick = el<HTMLInputElement>("#switch-on-click");
const autostart = el<HTMLInputElement>("#autostart");
const autoSwitch = el<HTMLInputElement>("#auto-switch");
const autoSwitchNote = el<HTMLParagraphElement>("#auto-switch-note");
const autoSwitchExcludedBlock = el<HTMLDetailsElement>(
  "#auto-switch-excluded-block",
);
const autoSwitchExcluded = el<HTMLInputElement>("#auto-switch-excluded");
const autoSwitchExcludedSave = el<HTMLButtonElement>(
  "#auto-switch-excluded-save",
);
const statusEl = el<HTMLSpanElement>("#status");

let state: AppState | null = null;
let recording = false;

function setStatus(message: string, isError = false): void {
  statusEl.textContent = message;
  statusEl.classList.toggle("error", isError);
}

function layoutName(id: string): string {
  return state?.available.find((l) => l.id === id)?.name ?? id;
}

function formatHotkey(hotkey: string): string {
  if (!hotkey) return "— not set —";
  const isMac = navigator.userAgent.includes("Mac");
  const labels: Record<string, string> = {
    CmdOrCtrl: isMac ? "⌘" : "Ctrl",
    Control: "Ctrl",
    Alt: "Alt",
    Option: "Option",
    Shift: "Shift",
    Super: "Super",
    Meta: "Super",
  };
  return hotkey
    .split("+")
    .map((part) => labels[part] ?? part)
    .join("+");
}

function renderBanner(): void {
  // The platform limitation matters most; a hotkey clash is worth reporting too.
  const message = state
    ? state.unsupportedReason ?? state.hotkeyError
    : null;

  banner.hidden = message === null;
  if (message) banner.textContent = message;
}

function renderCurrent(): void {
  const current = state?.current ?? null;
  currentLayoutEl.textContent = current ? layoutName(current) : "—";
}

function renderHotkey(): void {
  hotkeyBtn.textContent = recording
    ? "Нажмите комбинацию…"
    : formatHotkey(state?.config.hotkey ?? "");
}

function renderSwitches(): void {
  if (!state) return;
  switchOnClick.checked = state.config.switchOnClick;
  autostart.checked = state.config.autostart;
  cycleBtn.disabled = !state.supported || state.config.layouts.length < 2;
}

function renderAutoSwitch(): void {
  if (!state) return;

  autoSwitch.checked = state.config.autoSwitch;
  autoSwitch.disabled = !state.autoSwitchAvailable;

  // Two different problems, so two different messages: the feature is not
  // built for this platform, or it failed to start here and now.
  const note = !state.autoSwitchAvailable
    ? "Автоматическое переключение работает только на Windows"
    : state.autoSwitchError;

  autoSwitchNote.hidden = note === null;
  if (note) autoSwitchNote.textContent = note;

  autoSwitchExcludedBlock.hidden = !state.autoSwitchAvailable;
  if (document.activeElement !== autoSwitchExcluded) {
    autoSwitchExcluded.value = state.config.autoSwitchExcludedApps.join(", ");
  }
}

function makeLayoutItem(layout: LayoutInfo): HTMLLIElement {
  const enabled = state!.config.layouts.includes(layout.id);

  const li = document.createElement("li");
  li.className = enabled ? "layout-item" : "layout-item disabled";
  li.draggable = true;
  li.dataset.id = layout.id;

  const grip = document.createElement("span");
  grip.className = "grip";
  grip.textContent = "⋮⋮";
  li.appendChild(grip);

  const checkbox = document.createElement("input");
  checkbox.type = "checkbox";
  checkbox.checked = enabled;
  checkbox.setAttribute("aria-label", `Включить раскладку ${layout.name}`);
  li.appendChild(checkbox);

  const name = document.createElement("span");
  name.className = "layout-name";
  name.textContent = layout.name;
  li.appendChild(name);

  if (state?.current === layout.id) {
    const badge = document.createElement("span");
    badge.className = "badge-active";
    badge.textContent = "активна";
    li.appendChild(badge);
  }

  wireDragAndDrop(li, checkbox);
  return li;
}

function wireDragAndDrop(li: HTMLLIElement, checkbox: HTMLInputElement): void {
  let dragEnabled = true;

  li.addEventListener("mousedown", (event) => {
    // Keep the checkbox clickable: only the row itself starts a drag.
    dragEnabled = !(event.target as HTMLElement).closest("input");
  });

  li.addEventListener("dragstart", (event) => {
    if (!dragEnabled) {
      event.preventDefault();
      return;
    }
    li.classList.add("dragging");
    event.dataTransfer?.setData("text/plain", li.dataset.id ?? "");
    if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  });

  li.addEventListener("dragend", () => {
    li.classList.remove("dragging");
    layoutList.querySelectorAll(".drag-over").forEach((n) => {
      n.classList.remove("drag-over");
    });
  });

  li.addEventListener("dragover", (event) => {
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
    li.classList.add("drag-over");
  });

  li.addEventListener("dragleave", () => li.classList.remove("drag-over"));

  li.addEventListener("drop", (event) => {
    event.preventDefault();
    li.classList.remove("drag-over");
    const draggedId = event.dataTransfer?.getData("text/plain");
    const targetId = li.dataset.id;
    if (!draggedId || !targetId || draggedId === targetId) return;

    const order = layoutOrder();
    const from = order.indexOf(draggedId);
    const to = order.indexOf(targetId);
    if (from === -1 || to === -1) return;

    order.splice(to, 0, ...order.splice(from, 1));
    void saveLayoutOrder(order);
  });

  checkbox.addEventListener("change", () => {
    const order = layoutOrder();
    if (checkbox.checked) {
      if (!order.includes(li.dataset.id!)) order.push(li.dataset.id!);
    } else {
      const at = order.indexOf(li.dataset.id!);
      if (at !== -1) order.splice(at, 1);
    }
    void saveLayoutOrder(order);
  });

  li.addEventListener("dblclick", (event) => {
    if ((event.target as HTMLElement).closest("input")) return;
    void runCommand("cycle_layout").catch((err) => setStatus(String(err), true));
  });
}

/** Cycle order of every visible layout, enabled or not (for reordering). */
function layoutOrder(): string[] {
  return Array.from(layoutList.querySelectorAll<HTMLLIElement>(".layout-item")).map(
    (li) => li.dataset.id!,
  );
}

function renderLayouts(): void {
  layoutList.replaceChildren();
  const available = state?.available ?? [];
  layoutEmpty.hidden = available.length > 0;

  // Enabled layouts first (in cycle order), then the remaining ones.
  const enabledOrder = state?.config.layouts ?? [];
  const sorted = [
    ...enabledOrder
      .map((id) => available.find((l) => l.id === id))
      .filter((l): l is LayoutInfo => Boolean(l)),
    ...available.filter((l) => !enabledOrder.includes(l.id)),
  ];

  sorted.forEach((layout) => {
    layoutList.appendChild(makeLayoutItem(layout));
  });
}

function render(): void {
  renderBanner();
  renderCurrent();
  renderHotkey();
  renderSwitches();
  renderAutoSwitch();
  renderLayouts();
}

async function refresh(): Promise<void> {
  state = await invoke<AppState>("get_state");
  render();
}

async function saveLayoutOrder(order: string[]): Promise<void> {
  state = await invoke<AppState>("set_layouts", { layouts: order });
  render();
}

async function runCommand<T = AppState>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const result = await invoke<T>(command, args);
  if (result !== undefined) {
    state = result as AppState;
    render();
  }
  return result;
}

function isModifier(code: string): boolean {
  return [
    "ControlLeft",
    "ControlRight",
    "ShiftLeft",
    "ShiftRight",
    "AltLeft",
    "AltRight",
    "MetaLeft",
    "MetaRight",
    "OSLeft",
    "OSRight",
  ].includes(code);
}

/** Builds a Tauri accelerator string such as `CmdOrCtrl+Shift+Space`. */
function acceleratorFromEvent(event: KeyboardEvent): string | null {
  if (isModifier(event.code)) return null;

  const parts: string[] = [];
  if (event.metaKey) parts.push("CmdOrCtrl");
  else if (event.ctrlKey) parts.push("Control");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  parts.push(event.code);

  return parts.join("+");
}

function startRecording(): void {
  recording = true;
  renderHotkey();
  hotkeyBtn.classList.add("recording");
  hotkeyBtn.focus();
}

function stopRecording(): void {
  recording = false;
  hotkeyBtn.classList.remove("recording");
  renderHotkey();
}

hotkeyBtn.addEventListener("click", startRecording);

hotkeyBtn.addEventListener("keydown", async (event) => {
  if (!recording) return;
  event.preventDefault();

  if (event.code === "Escape") {
    stopRecording();
    return;
  }

  const accelerator = acceleratorFromEvent(event);
  if (!accelerator) return;

  stopRecording();
  try {
    await runCommand("set_hotkey", { hotkey: accelerator });
    setStatus(`Горячая клавиша: ${formatHotkey(accelerator)}`);
  } catch (err) {
    setStatus(`Не удалось установить горячую клавишу: ${String(err)}`, true);
  }
});

hotkeyBtn.addEventListener("blur", stopRecording);

hotkeyResetBtn.addEventListener("click", async () => {
  try {
    await runCommand("set_hotkey", { hotkey: DEFAULT_HOTKEY });
    setStatus(`Горячая клавиша сброшена: ${formatHotkey(DEFAULT_HOTKEY)}`);
  } catch (err) {
    setStatus(String(err), true);
  }
});

cycleBtn.addEventListener("click", async () => {
  try {
    await runCommand("cycle_layout");
    setStatus("Раскладка переключена");
  } catch (err) {
    setStatus(String(err), true);
  }
});

switchOnClick.addEventListener("change", async () => {
  const value = switchOnClick.checked;
  try {
    await runCommand("set_switch_on_click", { value });
  } catch (err) {
    switchOnClick.checked = !value;
    setStatus(String(err), true);
  }
});

autostart.addEventListener("change", async () => {
  const value = autostart.checked;
  try {
    await runCommand("set_autostart", { value });
  } catch (err) {
    autostart.checked = !value;
    setStatus(String(err), true);
  }
});

autoSwitch.addEventListener("change", async () => {
  const value = autoSwitch.checked;
  try {
    state = await runCommand<AppState>("set_auto_switch", { value });
    renderAutoSwitch();
    setStatus(
      value
        ? "Автоматическое переключение включено"
        : "Автоматическое переключение выключено",
    );
  } catch (err) {
    autoSwitch.checked = !value;
    setStatus(String(err), true);
  }
});

autoSwitchExcludedSave.addEventListener("click", async () => {
  const apps = autoSwitchExcluded.value
    .split(",")
    .map((name) => name.trim())
    .filter((name) => name.length > 0);

  try {
    state = await runCommand<AppState>("set_auto_switch_excluded_apps", {
      apps,
    });
    renderAutoSwitch();
    setStatus("Список исключений сохранён");
  } catch (err) {
    setStatus(String(err), true);
  }
});

window.addEventListener("DOMContentLoaded", () => {
  void (async () => {
    try {
      await refresh();
    } catch (err) {
      setStatus(`Не удалось загрузить состояние: ${String(err)}`, true);
    }

    await listen<{ id: string | null }>("layout-changed", (event) => {
      if (state) {
        state.current = event.payload.id;
        renderCurrent();
        renderLayouts();
      }
    });
  })();
});