import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import enUS from "../locales/en-US.json";
import zhCN from "../locales/zh-CN.json";

export type UiLanguage = "zh-CN" | "en-US";
export type LanguagePreference = "" | UiLanguage;

const STORAGE_KEY = "deploycode:language";

export const resources = {
  "zh-CN": { translation: zhCN },
  "en-US": { translation: enUS },
} as const;

function systemLanguage(): UiLanguage {
  const lang = (navigator.language || "").toLowerCase();
  return lang.startsWith("zh") ? "zh-CN" : "en-US";
}

function readCachedPreference(): LanguagePreference | null {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    if (value === "" || value === "zh-CN" || value === "en-US") return value;
  } catch {
    /* ignore */
  }
  return null;
}

export function normalizeLanguagePreference(value: string | null | undefined): LanguagePreference {
  return value === "zh-CN" || value === "en-US" ? value : "";
}

export function resolveLanguage(preference: LanguagePreference | null | undefined): UiLanguage {
  if (preference === "zh-CN" || preference === "en-US") return preference;
  if (preference === "") return systemLanguage();
  const cached = readCachedPreference();
  if (cached === "zh-CN" || cached === "en-US") return cached;
  return systemLanguage();
}

function syncWindowTitle() {
  const title = i18n.t("app.windowTitle");
  document.title = title;
  if ("__TAURI_INTERNALS__" in window) {
    void getCurrentWindow()
      .setTitle(title)
      .catch(() => undefined);
  }
}

/** 托盘菜单文案跟随界面语言（浏览器调试模式下忽略）。 */
function syncTrayLanguage(target: UiLanguage) {
  if (!("__TAURI_INTERNALS__" in window)) return;
  void invoke("set_tray_language", { language: target }).catch(() => undefined);
}

export function applyLanguage(preference: string | null | undefined) {
  const normalized = normalizeLanguagePreference(preference);
  const target: UiLanguage = normalized === "" ? systemLanguage() : normalized;
  try {
    localStorage.setItem(STORAGE_KEY, normalized);
  } catch {
    /* ignore */
  }
  document.documentElement.lang = target;
  void i18n.changeLanguage(target);
  syncTrayLanguage(target);
  syncWindowTitle();
}

void i18n.use(initReactI18next).init({
  resources,
  lng: resolveLanguage(readCachedPreference()),
  fallbackLng: "en-US",
  initAsync: false,
  interpolation: { escapeValue: false },
  returnNull: false,
});

i18n.on("languageChanged", syncWindowTitle);

syncWindowTitle();

export default i18n;
