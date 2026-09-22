import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  api: {
    listRepos: vi.fn(async () => []),
    startDeployConfig: vi.fn(async () => "d1"),
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

  it("按配置启动服务器部署：参数取自配置 id，建立 running 状态", async () => {
    const id = await useApp.getState().startDeployConfig("config-1");
    expect(id).toBe("d1");
    expect(api.startDeployConfig).toHaveBeenCalledWith("config-1");
    expect(useApp.getState().live).toEqual({
      recordId: "d1",
      lines: [],
      progress: 0,
      status: "running",
      record: null,
    });
  });

  it("服务器部署进行中时拒绝再次启动", async () => {
    useApp.setState({ live: { recordId: "d1", lines: [], progress: 0, status: "running", record: null } });
    await expect(useApp.getState().startDeployConfig("config-1")).rejects.toThrow();
    expect(api.startDeployConfig).not.toHaveBeenCalled();
  });

  it("已有任务运行时拒绝并发启动", async () => {
    useApp.setState({ livePages: { recordId: "p1", lines: [], progress: 0, status: "running", record: null } });
    await expect(useApp.getState().startPagesDeploy({ repoId: "repo", skipBuild: false })).rejects.toThrow();
    expect(api.startPagesDeploy).not.toHaveBeenCalled();
  });

  it("并发 refreshRepos 复用同一个请求（轮询与手动点击撞车时只拉一次）", async () => {
    let calls = 0;
    vi.mocked(api.listRepos).mockImplementation(async () => {
      calls += 1;
      await new Promise((resolve) => setTimeout(resolve, 5));
      return [];
    });
    await Promise.all([
      useApp.getState().refreshRepos(true),
      useApp.getState().refreshRepos(true),
    ]);
    expect(calls).toBe(1);
  });

  it("silent 刷新失败不弹 toast，普通刷新失败要提示", async () => {
    vi.mocked(api.listRepos).mockRejectedValueOnce(new Error("读取失败"));
    await useApp.getState().refreshRepos(true);
    expect(useApp.getState().toasts).toHaveLength(0);

    vi.mocked(api.listRepos).mockRejectedValueOnce(new Error("读取失败"));
    await useApp.getState().refreshRepos();
    expect(useApp.getState().toasts).toHaveLength(1);
  });
});
