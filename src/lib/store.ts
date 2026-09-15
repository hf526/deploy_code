import { create } from "zustand";

import { api } from "./api";
import i18n from "./i18n";
import { applyTaskEvent, reconcileLiveTask } from "./liveTask";
import type {
  BackupConfig,
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
  githubToken: "",
  pagesHistoryLimit: 200,
  language: "",
  atomicRelease: false,
  releaseKeep: 5,
  scheduledBackupEnabled: false,
  scheduledBackupTime: "03:00",
  scheduledBackupConfigId: null,
};

let toastSeq = 0;

interface AppStore {
  ready: boolean;
  repos: RepoInfo[];
  tabs: string[];
  servers: ServerConfig[];
  settings: Settings;
  history: DeployRecord[];
  backups: BackupRecord[];
  backupTargets: BackupTarget[];
  backupConfigs: BackupConfig[];
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
  refreshBackupConfigs: () => Promise<void>;
  refreshPagesRecords: (repoId?: string | null) => Promise<void>;
  setSettings: (settings: Settings) => void;

  toast: (kind: Toast["kind"], message: string) => void;
  dismissToast: (id: number) => void;

  startDeploy: (request: DeployRequest) => Promise<string>;
  redeploy: (recordId: string) => Promise<string>;
  cancelDeploy: () => Promise<void>;
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
  backupConfigs: [],
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
      configsResult,
      pagesResult,
    ] = await Promise.allSettled([
      api.listRepos(),
      api.listServers(),
      api.getSettings(),
      api.listHistory(),
      api.listBackups(),
      api.listBackupTargets(),
      api.listBackupConfigs(),
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
      ...(configsResult.status === "fulfilled" ? { backupConfigs: configsResult.value } : {}),
      ...(pagesResult.status === "fulfilled" ? { pagesRecords: pagesResult.value } : {}),
    });
    const failures = [
      reposResult,
      serversResult,
      settingsResult,
      historyResult,
      backupsResult,
      targetsResult,
      configsResult,
      pagesResult,
    ].filter((result): result is PromiseRejectedResult => result.status === "rejected");
    if (failures.length > 0) {
      // 汇总所有失败，避免只看到第一个出错接口而忽略后面的。
      const message = failures.map((result) => String(result.reason)).join("\n");
      get().toast("error", message);
    }

    // 对账后台任务：重载后可能错过 finished/started 事件，用持久化记录收敛，避免 live 永久卡在 running。
    // history 等列表接口返回的是「新 -> 旧」，find 直接取最近一条即可。
    const { live: currentLive, liveBackup: currentBackup, livePages: currentPages } = get();
    const nextLive = reconcileLiveTask(
      currentLive,
      historyResult.status === "fulfilled" ? historyResult.value : null,
    );
    if (nextLive !== undefined) set({ live: nextLive });
    const nextBackup = reconcileLiveTask(
      currentBackup,
      backupsResult.status === "fulfilled" ? backupsResult.value : null,
    );
    if (nextBackup !== undefined) set({ liveBackup: nextBackup });
    const nextPages = reconcileLiveTask(
      currentPages,
      pagesResult.status === "fulfilled" ? pagesResult.value : null,
    );
    if (nextPages !== undefined) set({ livePages: nextPages });
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
    set((state) => {
      // 最近查看的仓库移到末尾，返回工作区时可直接回到它。
      if (state.tabs[state.tabs.length - 1] === repoId) return {};
      return { tabs: [...state.tabs.filter((id) => id !== repoId), repoId] };
    }),

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

  refreshBackupConfigs: async () => {
    try {
      set({ backupConfigs: await api.listBackupConfigs() });
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
      const message = i18n.t("deploy.toast.running");
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
      const message = i18n.t("deploy.toast.running");
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

  cancelDeploy: async () => {
    const { live, history } = get();
    if (!live || live.status !== "running") return;
    // 重载后 live.recordId 可能为空：从历史记录的 running 项兜底解析。
    const recordId = live.recordId || history.find((item) => item.status === "running")?.id;
    if (!recordId) return;
    try {
      await api.cancelDeploy(recordId);
      // 后端会推送 finished 事件；这里先收敛状态，避免按钮停留在 running。
      // 只在仍是同一任务时收敛，避免覆盖期间开始的其它任务状态。
      set((state) =>
        state.live?.status === "running" &&
        (!state.live.recordId || state.live.recordId === recordId)
          ? { live: { ...state.live, recordId, status: "failed", progress: 100 } }
          : {},
      );
    } catch (error) {
      get().toast("error", String(error));
    }
  },

  handleDeployEvent: (event) => {
    applyTaskEvent<DeployRecord>(event, {
      getLive: () => get().live,
      setLive: (live) => set({ live }),
      findRunningId: () => get().history.find((item) => item.status === "running")?.id ?? "",
      announce: (record) => {
        void get().refreshHistory();
        if (record.status === "success") {
          get().toast("success", i18n.t("deploy.toast.success", { name: record.repoName }));
        } else {
          get().toast(
            "error",
            i18n.t("deploy.toast.failed", {
              name: record.repoName,
              error: record.error ?? i18n.t("common.unknownError"),
            }),
          );
        }
      },
    });
  },

  clearLive: () => set({ live: null }),

  startBackup: async (request) => {
    if (get().liveBackup?.status === "running") {
      const message = i18n.t("backup.toast.running");
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
    applyTaskEvent<BackupRecord>(event, {
      getLive: () => get().liveBackup,
      setLive: (live) => set({ liveBackup: live }),
      findRunningId: () => get().backups.find((item) => item.status === "running")?.id ?? "",
      announce: (record) => {
        void get().refreshBackups();
        const name = `${record.serverName} · ${record.database}`;
        if (record.status === "success") {
          get().toast("success", i18n.t("backup.toast.success", { name }));
        } else {
          get().toast(
            "error",
            i18n.t("backup.toast.failed", { name, error: record.error ?? i18n.t("common.unknownError") }),
          );
        }
      },
    });
  },

  clearLiveBackup: () => set({ liveBackup: null }),

  startPagesDeploy: async (request) => {
    if (get().livePages?.status === "running") {
      const message = i18n.t("pages.toast.running");
      get().toast("error", message);
      throw new Error(message);
    }
    set({
      livePages: { recordId: "", lines: [], progress: 0, status: "running", record: null },
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
    applyTaskEvent<PagesDeployRecord>(event, {
      getLive: () => get().livePages,
      setLive: (live) => set({ livePages: live }),
      findRunningId: () => get().pagesRecords.find((item) => item.status === "running")?.id ?? "",
      announce: (record) => {
        void get().refreshPagesRecords();
        const name = `${record.repoName} -> ${record.projectName}`;
        if (record.status === "success") {
          get().toast(
            "success",
            record.url
              ? i18n.t("pages.toast.successUrl", { name, url: record.url })
              : i18n.t("pages.toast.success", { name }),
          );
        } else {
          get().toast(
            "error",
            i18n.t("pages.toast.failed", { name, error: record.error ?? i18n.t("common.unknownError") }),
          );
        }
      },
    });
  },

  clearLivePages: () => set({ livePages: null }),
}));
