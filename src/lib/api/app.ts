import { invoke } from "@tauri-apps/api/core";

import type {
  ImportPreview,
  Settings,
  ShutdownStatus,
} from "../types";

export const appCommands = {
  // 应用
  getDataDir: () => invoke<string>("get_data_dir"),
  getAppVersion: () => invoke<string>("get_app_version"),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  exportConfig: () => invoke<string>("export_config"),
  previewConfigImport: (jsonStr: string) =>
    invoke<ImportPreview>("preview_config_import", { jsonStr }),
  importConfig: (jsonStr: string) => invoke<ImportPreview>("import_config", { jsonStr }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  /** 终端里点到的链接交给系统浏览器：webview 自己开不了新窗口（wry 默认吞掉那个请求）。 */
  openExternalUrl: (url: string) => invoke<void>("open_external_url", { url }),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => invoke<void>("set_autostart", { enabled }),
  getShutdownStatus: () => invoke<ShutdownStatus>("get_shutdown_status"),
  scheduleShutdown: (minutes: number) =>
    invoke<ShutdownStatus>("schedule_shutdown", { minutes }),
  cancelShutdown: () => invoke<ShutdownStatus>("cancel_shutdown"),
};
