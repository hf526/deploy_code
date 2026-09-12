import { useSyncExternalStore } from "react";

export type ThemeMode = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "deploycode:theme";

const listeners = new Set<() => void>();
let currentMode: ThemeMode = readStored();

function readStored(): ThemeMode {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    if (value === "light" || value === "dark" || value === "system") return value;
  } catch {
    /* ignore */
  }
  return "system";
}

function systemTheme(): ResolvedTheme {
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function resolveTheme(mode: ThemeMode): ResolvedTheme {
  return mode === "system" ? systemTheme() : mode;
}

export function applyTheme(mode: ThemeMode = currentMode) {
  document.documentElement.dataset.theme = resolveTheme(mode);
}

export function getThemeMode(): ThemeMode {
  return currentMode;
}

export function setThemeMode(mode: ThemeMode) {
  if (mode === currentMode) return;
  currentMode = mode;
  try {
    localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    /* ignore */
  }
  applyTheme(mode);
  listeners.forEach((emit) => emit());
}

window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
  if (currentMode === "system") {
    applyTheme();
    listeners.forEach((emit) => emit());
  }
});

applyTheme(currentMode);

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** 组件内读取 / 修改主题。 */
export function useTheme(): [ThemeMode, (mode: ThemeMode) => void] {
  const mode = useSyncExternalStore(subscribe, getThemeMode);
  return [mode, setThemeMode];
}
