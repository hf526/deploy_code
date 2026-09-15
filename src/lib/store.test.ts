import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  api: {
    startPagesDeploy: vi.fn(async () => "p1"),
    listHistory: vi.fn(async () => []),
    listBackups: vi.fn(async () => []),
    listPagesRecords: vi.fn(async () => []),
  },
}));
vi.mock("./i18n", () => ({ default: { t: (key: string) => key } }));

import { api } from "./api";
import type { PagesDeployRecord } from "./types";
import { useApp } from "./store";

// toast 内部用 window.setTimeout 自动消失；node 环境用桩即可。
vi.stubGlobal("window", { setTimeout: () => 0 });

function pagesRecord(status: PagesDeployRecord["status"]): PagesDeployRecord {
  return {
    id: "p1",
    provider: "cloudflare",
    repoId: "repo",
    repoName: "repo",
    projectName: "site",
    branch: "main",
    commit: "abc",
    commitShort: "abc",
    status,
    error: null,
    log: "",
    url: null,
    startedAt: "",
    finishedAt: null,
    durationMs: 0,
  };
}

beforeEach(() => {
  useApp.setState({
    live: null,
    liveBackup: null,
    livePages: null,
    history: [],
    backups: [],
    pagesRecords: [],
    toasts: [],
  });
  vi.clearAllMocks();
});

describe("store 长任务接线", () => {
  it("部署事件写入 live：日志追加、完成后收敛状态并刷新历史", () => {
    useApp.setState({ live: { recordId: "d1", lines: [], progress: 0, status: "running", record: null } });
    useApp.getState().handleDeployEvent({ type: "log", level: "info", message: "编译中" });
    expect(useApp.getState().live?.lines).toEqual([{ level: "info", message: "编译中" }]);

    const record = {
      id: "d1",
      repoId: "repo",
      repoName: "repo",
      rev: "main",
      branch: "main",
      worktree: false,
      atomicRelease: false,
      releaseDir: null,
      commit: "abc",
      commitShort: "abc",
      commitSubject: "init",
      serverId: "s1",
      serverName: "prod",
      targetDir: "/opt/app",
      scriptDir: "docker",
      scripts: [],
      runScripts: true,
      envFiles: [],
      status: "success" as const,
      error: null,
      log: "",
      startedAt: "",
      finishedAt: null,
      durationMs: 0,
    };
    useApp.getState().handleDeployEvent({ type: "finished", record });
    expect(useApp.getState().live?.status).toBe("success");
    expect(useApp.getState().live?.progress).toBe(100);
    expect(useApp.getState().live?.record).toEqual(record);
    expect(api.listHistory).toHaveBeenCalled();
  });

  it("Pages 事件归约到 livePages（保证记录类型映射不错位）", () => {
    useApp.setState({ livePages: { recordId: "p1", lines: [], progress: 0, status: "running", record: null } });
    useApp.getState().handlePagesEvent({ type: "finished", record: pagesRecord("failed") });
    expect(useApp.getState().livePages?.status).toBe("failed");
    expect(useApp.getState().livePages?.record?.projectName).toBe("site");
    expect(useApp.getState().live).toBeNull();
  });

  it("启动 Pages 部署先建立 running 状态并记录返回的 id", async () => {
    const id = await useApp.getState().startPagesDeploy({ repoId: "repo", skipBuild: false });
    expect(id).toBe("p1");
    expect(useApp.getState().livePages).toEqual({
      recordId: "p1",
      lines: [],
      progress: 0,
      status: "running",
      record: null,
    });
  });

  it("已有任务运行时拒绝并发启动", async () => {
    useApp.setState({ livePages: { recordId: "p1", lines: [], progress: 0, status: "running", record: null } });
    await expect(useApp.getState().startPagesDeploy({ repoId: "repo", skipBuild: false })).rejects.toThrow();
    expect(api.startPagesDeploy).not.toHaveBeenCalled();
  });
});
