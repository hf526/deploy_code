import { create } from "zustand";

import { api } from "./api";
import type {
  DeployEvent,
  DeployRecord,
  DeployRequest,
  LiveDeploy,
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
  toasts: Toast[];
  live: LiveDeploy | null;

  loadAll: () => Promise<void>;
  refreshRepos: () => Promise<void>;
  addRepoFromPath: (path: string) => Promise<RepoInfo | null>;
  openTab: (repoId: string) => void;
  closeTab: (repoId: string) => void;
  refreshServers: () => Promise<void>;
  refreshHistory: (repoId?: string | null) => Promise<void>;
  setSettings: (settings: Settings) => void;

  toast: (kind: Toast["kind"], message: string) => void;
  dismissToast: (id: number) => void;

  startDeploy: (request: DeployRequest) => Promise<string>;
  redeploy: (recordId: string) => Promise<string>;
  handleDeployEvent: (event: DeployEvent) => void;
  clearLive: () => void;
}

export const useApp = create<AppStore>((set, get) => ({
  ready: false,
  repos: [],
  tabs: [],
  servers: [],
  settings: defaultSettings,
  history: [],
  toasts: [],
  live: null,

  loadAll: async () => {
    // 各接口独立处理，避免单个失败（如历史文件损坏）导致设置/服务器全部回退成默认值。
    const [reposResult, serversResult, settingsResult, historyResult] = await Promise.allSettled([
      api.listRepos(),
      api.listServers(),
      api.getSettings(),
      api.listHistory(),
    ]);
    set({
      ready: true,
      ...(reposResult.status === "fulfilled" ? { repos: reposResult.value } : {}),
      ...(serversResult.status === "fulfilled" ? { servers: serversResult.value } : {}),
      ...(settingsResult.status === "fulfilled" ? { settings: settingsResult.value } : {}),
      ...(historyResult.status === "fulfilled" ? { history: historyResult.value } : {}),
    });
    const failed = [reposResult, serversResult, settingsResult, historyResult].find(
      (result) => result.status === "rejected",
    );
    if (failed && failed.status === "rejected") {
      get().toast("error", String(failed.reason));
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
}));
