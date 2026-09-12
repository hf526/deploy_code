import { create } from "zustand";

import { api } from "./api";
import type {
  BackupEvent,
  BackupRecord,
  BackupRequest,
  BackupTarget,
  DeployEvent,
  DeployRecord,
  DeployRequest,
  LiveBackup,
  LiveDeploy,
  LivePages,
  PagesDeployRecord,
  PagesEvent,
  PagesRequest,
  RepoInfo,
  ServerConfig,
  Settings,
} from "./types";

export interface Toast {
  id: number;
  kind: "success" | "error" | "info";
  message: string;
}

export const defaultSettings: Settings = {
  scriptDir: "docker",
  runScripts: true,
  connectTimeoutSecs: 15,
  scriptTimeoutSecs: 1800,
  keepRemoteArchive: false,
  historyLimit: 500,
  supabaseUrl: "",
  defaultBackupTargetId: null,
  backupHistoryLimit: 200,
  backupTimeoutSecs: 3600,
  cloudflareApiToken: "",
  cloudflareAccountId: "",
  pagesHistoryLimit: 200,
};

let toastSeq = 0;
const MAX_LIVE_LINES = 6000;

interface AppStore {
  ready: boolean;
  repos: RepoInfo[];
  tabs: string[];
  servers: ServerConfig[];
  settings: Settings;
  history: DeployRecord[];
  backups: BackupRecord[];
  backupTargets: BackupTarget[];
  pagesRecords: PagesDeployRecord[];
  toasts: Toast[];
  live: LiveDeploy | null;
  liveBackup: LiveBackup | null;
  livePages: LivePages | null;

  loadAll: () => Promise<void>;
  refreshRepos: () => Promise<void>;
  addRepoFromPath: (path: string) => Promise<RepoInfo | null>;
  openTab: (repoId: string) => void;
  closeTab: (repoId: string) => void;
  refreshServers: () => Promise<void>;
  refreshHistory: (repoId?: string | null) => Promise<void>;
  refreshBackups: (serverId?: string | null) => Promise<void>;
  refreshBackupTargets: () => Promise<void>;
  refreshPagesRecords: (repoId?: string | null) => Promise<void>;
  setSettings: (settings: Settings) => void;

  toast: (kind: Toast["kind"], message: string) => void;
  dismissToast: (id: number) => void;

  startDeploy: (request: DeployRequest) => Promise<string>;
  redeploy: (recordId: string) => Promise<string>;
  handleDeployEvent: (event: DeployEvent) => void;
  clearLive: () => void;

  startBackup: (request: BackupRequest) => Promise<string>;
  handleBackupEvent: (event: BackupEvent) => void;
  clearLiveBackup: () => void;

  startPagesDeploy: (request: PagesRequest) => Promise<string>;
  handlePagesEvent: (event: PagesEvent) => void;
  clearLivePages: () => void;
}

export const useApp = create<AppStore>((set, get) => ({
  ready: false,
  repos: [],
  tabs: [],
  servers: [],
  settings: defaultSettings,
  history: [],
  backups: [],
  backupTargets: [],
  pagesRecords: [],
  toasts: [],
  live: null,
  liveBackup: null,
  livePages: null,

  loadAll: async () => {
    // 各接口独立处理，避免单个失败（如历史文件损坏）导致设置/服务器全部回退成默认值。
    const [
      reposResult,
      serversResult,
      settingsResult,
      historyResult,
      backupsResult,
      targetsResult,
      pagesResult,
    ] = await Promise.allSettled([
      api.listRepos(),
      api.listServers(),
      api.getSettings(),
      api.listHistory(),
      api.listBackups(),
      api.listBackupTargets(),
      api.listPagesRecords(),
    ]);
    set({
      ready: true,
      ...(reposResult.status === "fulfilled" ? { repos: reposResult.value } : {}),
      ...(serversResult.status === "fulfilled" ? { servers: serversResult.value } : {}),
      ...(settingsResult.status === "fulfilled" ? { settings: settingsResult.value } : {}),
      ...(historyResult.status === "fulfilled" ? { history: historyResult.value } : {}),
      ...(backupsResult.status === "fulfilled" ? { backups: backupsResult.value } : {}),
      ...(targetsResult.status === "fulfilled" ? { backupTargets: targetsResult.value } : {}),
      ...(pagesResult.status === "fulfilled" ? { pagesRecords: pagesResult.value } : {}),
    });
    const failures = [
      reposResult,
      serversResult,
      settingsResult,
      historyResult,
      backupsResult,
      targetsResult,
      pagesResult,
    ].filter((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failures.length > 0) {
      // 汇总所有失败，避免只看到第一个出错接口而忽略后面的。
      const message = failures.map((result) => String(result.reason)).join("；");
      get().toast("error", message);
    }
  },

  refreshRepos: async () => {
    try {
      const repos = await api.listRepos();
      set((state) => ({
        repos,
        tabs: state.tabs.filter((id) => repos.some((repo) => repo.id === id)),
      }));
    } catch (error) {
      get().toast("error", String(error));
    }
  },

  openTab: (repoId) =>
    set((state) =>
      state.tabs.includes(repoId) ? {} : { tabs: [...state.tabs, repoId] },
    ),

  closeTab: (repoId) =>
    set((state) => ({ tabs: state.tabs.filter((id) => id !== repoId) })),

  addRepoFromPath: async (path) => {
    try {
      const repo = await api.addRepo({ path });
      await get().refreshRepos();
      return repo;
    } catch (error) {
      get().toast("error", String(error));
      return null;
    }
  },

  refreshServers: async () => {
    try {
      set({ servers: await api.listServers() });
    } catch (error) {
      get().toast("error", String(error));
    }
  },

  refreshHistory: async (repoId) => {
    try {
      set({ history: await api.listHistory(repoId ?? null) });
    } catch (error) {
      get().toast("error", String(error));
    }
  },

  refreshBackups: async (serverId) => {
    try {
      set({ backups: await api.listBackups(serverId ?? null) });
    } catch (error) {
      get().toast("error", String(error));
    }
  },

  refreshBackupTargets: async () => {
    try {
      set({ backupTargets: await api.listBackupTargets() });
    } catch (error) {
      get().toast("error", String(error));
    }
  },

  refreshPagesRecords: async (repoId) => {
    try {
      set({ pagesRecords: await api.listPagesRecords(repoId ?? null) });
    } catch (error) {
      get().toast("error", String(error));
    }
  },

  setSettings: (settings) => set({ settings }),

  toast: (kind, message) => {
    const id = ++toastSeq;
    set((state) => ({ toasts: [...state.toasts, { id, kind, message }] }));
    window.setTimeout(() => get().dismissToast(id), kind === "error" ? 7000 : 3500);
  },

  dismissToast: (id) => set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) })),

  startDeploy: async (request) => {
    if (get().live?.status === "running") {
      const message = "已有部署正在进行，请等待其完成后再试";
      get().toast("error", message);
      throw new Error(message);
    }
    set({
      live: { recordId: "", lines: [], progress: 0, status: "running", record: null },
    });
    try {
      const recordId = await api.startDeploy(request);
      set((state) => (state.live ? { live: { ...state.live, recordId } } : {}));
      return recordId;
    } catch (error) {
      set({ live: null });
      get().toast("error", String(error));
      throw error;
    }
  },

  redeploy: async (recordId) => {
    if (get().live?.status === "running") {
      const message = "已有部署正在进行，请等待其完成后再试";
      get().toast("error", message);
      throw new Error(message);
    }
    set({ live: { recordId: "", lines: [], progress: 0, status: "running", record: null } });
    try {
      const newId = await api.redeploy(recordId);
      set((state) => (state.live ? { live: { ...state.live, recordId: newId } } : {}));
      return newId;
    } catch (error) {
      set({ live: null });
      get().toast("error", String(error));
      throw error;
    }
  },

  handleDeployEvent: (event) => {
    const { live } = get();

    const announce = (record: DeployRecord) => {
      void get().refreshHistory();
      if (record.status === "success") {
        get().toast("success", `${record.repoName} 部署成功`);
      } else {
        get().toast("error", `${record.repoName} 部署失败：${record.error ?? "未知错误"}`);
      }
    };

    if (!live) {
      // 刷新/重载后 live 为空，但后台部署仍在跑：至少刷新历史并提示结果。
      if (event.type === "finished") announce(event.record);
      return;
    }

    switch (event.type) {
      case "started":
        set({ live: { ...live, recordId: event.recordId } });
        break;
      case "log": {
        const lines = [...live.lines, { level: event.level, message: event.message }];
        if (lines.length > MAX_LIVE_LINES) lines.splice(0, lines.length - MAX_LIVE_LINES);
        set({ live: { ...live, lines } });
        break;
      }
      case "progress":
        set({ live: { ...live, progress: event.percent } });
        break;
      case "finished": {
        set({ live: { ...live, status: event.record.status, record: event.record, progress: 100 } });
        announce(event.record);
        break;
      }
    }
  },

  clearLive: () => set({ live: null }),

  startBackup: async (request) => {
    if (get().liveBackup?.status === "running") {
      const message = "已有备份正在进行，请等待其完成后再试";
      get().toast("error", message);
      throw new Error(message);
    }
    set({
      liveBackup: { recordId: "", lines: [], progress: 0, status: "running", record: null },
    });
    try {
      const recordId = await api.startBackup(request);
      set((state) =>
        state.liveBackup ? { liveBackup: { ...state.liveBackup, recordId } } : {},
      );
      return recordId;
    } catch (error) {
      set({ liveBackup: null });
      get().toast("error", String(error));
      throw error;
    }
  },

  handleBackupEvent: (event) => {
    const { liveBackup } = get();

    const announce = (record: BackupRecord) => {
      void get().refreshBackups();
      const name = `${record.serverName} · ${record.database}`;
      if (record.status === "success") {
        get().toast("success", `${name} 备份成功`);
      } else {
        get().toast("error", `${name} 备份失败：${record.error ?? "未知错误"}`);
      }
    };

    if (!liveBackup) {
      if (event.type === "finished") announce(event.record);
      return;
    }

    switch (event.type) {
      case "started":
        set({ liveBackup: { ...liveBackup, recordId: event.recordId } });
        break;
      case "log": {
        const lines = [...liveBackup.lines, { level: event.level, message: event.message }];
        if (lines.length > MAX_LIVE_LINES) lines.splice(0, lines.length - MAX_LIVE_LINES);
        set({ liveBackup: { ...liveBackup, lines } });
        break;
      }
      case "progress":
        set({ liveBackup: { ...liveBackup, progress: event.percent } });
        break;
      case "finished": {
        set({
          liveBackup: {
            ...liveBackup,
            status: event.record.status,
            record: event.record,
            progress: 100,
          },
        });
        announce(event.record);
        break;
      }
    }
  },

  clearLiveBackup: () => set({ liveBackup: null }),

  startPagesDeploy: async (request) => {
    if (get().livePages?.status === "running") {
      const message = "已有 Pages 部署正在进行，请等待其完成后再试";
      get().toast("error", message);
      throw new Error(message);
    }
    set({
      livePages: { recordId: "", lines: [], status: "running", record: null },
    });
    try {
      const recordId = await api.startPagesDeploy(request);
      set((state) => (state.livePages ? { livePages: { ...state.livePages, recordId } } : {}));
      return recordId;
    } catch (error) {
      set({ livePages: null });
      get().toast("error", String(error));
      throw error;
    }
  },

  handlePagesEvent: (event) => {
    const { livePages } = get();

    const announce = (record: PagesDeployRecord) => {
      void get().refreshPagesRecords();
      const name = `${record.repoName} -> ${record.projectName}`;
      if (record.status === "success") {
        get().toast("success", `${name} 部署成功${record.url ? `：${record.url}` : ""}`);
      } else {
        get().toast("error", `${name} 部署失败：${record.error ?? "未知错误"}`);
      }
    };

    if (!livePages) {
      // 重载/事件先到：把已开始的任务补进 live 状态，避免后续日志全部丢失。
      if (event.type === "started") {
        set({
          livePages: { recordId: event.recordId, lines: [], status: "running", record: null },
        });
        return;
      }
      if (event.type === "log") {
        set({
          livePages: {
            recordId: "",
            lines: [{ level: event.level, message: event.message }],
            status: "running",
            record: null,
          },
        });
        return;
      }
      announce(event.record);
      return;
    }

    switch (event.type) {
      case "started":
        set({ livePages: { ...livePages, recordId: event.recordId } });
        break;
      case "log": {
        const lines = [...livePages.lines, { level: event.level, message: event.message }];
        if (lines.length > MAX_LIVE_LINES) lines.splice(0, lines.length - MAX_LIVE_LINES);
        set({ livePages: { ...livePages, lines } });
        break;
      }
      case "finished": {
        set({ livePages: { ...livePages, status: event.record.status, record: event.record } });
        announce(event.record);
        break;
      }
    }
  },

  clearLivePages: () => set({ livePages: null }),
}));
