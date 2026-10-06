import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({
  api: {
    listRepos: vi.fn(async () => []),
    deployConfigTargets: vi.fn(async () => 2),
    startPagesDeploy: vi.fn(async () => "p1"),
    startContainerTransfer: vi.fn(async () => "c1"),
    restoreContainerBundle: vi.fn(async () => "c2"),
    startContainerConfigBackup: vi.fn(async () => "c3"),
    startBackup: vi.fn(async () => "db-1"),
    listHistory: vi.fn(async () => []),
    listBackups: vi.fn(async () => []),
    listPagesRecords: vi.fn(async () => []),
  },
}));
vi.mock("./i18n", () => ({ default: { t: (key: string) => key } }));

import { api } from "./api";
import type { ContainerRequest, PagesDeployRecord } from "./types";
import { useApp } from "./store";

// toast 内部用 window.setTimeout 自动消失；node 环境用桩即可。
vi.stubGlobal("window", { setTimeout: () => 0 });

function containerRequest(): ContainerRequest {
  return {
    serverId: "s1",
    project: "lf-blog",
    pauseSource: false,
    includeVolumes: true,
    includeImages: true,
    target: null,
  };
}

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
    liveContainer: null,
    history: [],
    backups: [],
    pagesRecords: [],
    toasts: [],
  });
  vi.clearAllMocks();
});

describe("store 长任务接线", () => {
  it("部署事件写入 live：日志追加、完成后收敛状态并刷新历史", () => {
    useApp.setState({ live: { recordId: "d1", lines: [], progress: 0, progressMessage: "", status: "running", record: null } });
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
    useApp.setState({ livePages: { recordId: "p1", lines: [], progress: 0, progressMessage: "", status: "running", record: null } });
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
      progressMessage: "",
      status: "running",
      record: null,
    });
  });

  it("按配置发起批量部署：目标随配置 id 一起提交，recordId 留给 started 事件补齐", async () => {
    const count = await useApp.getState().deployConfigTargets("config-1", ["s1", "s2"]);
    expect(count).toBe(2);
    expect(api.deployConfigTargets).toHaveBeenCalledWith("config-1", ["s1", "s2"]);
    expect(useApp.getState().live).toEqual({
      recordId: "",
      lines: [],
      progress: 0,
      progressMessage: "",
      status: "running",
      record: null,
    });
  });

  it("服务器部署进行中时拒绝再次启动", async () => {
    useApp.setState({ live: { recordId: "d1", lines: [], progress: 0, progressMessage: "", status: "running", record: null } });
    await expect(useApp.getState().deployConfigTargets("config-1", ["s1"])).rejects.toThrow();
    expect(api.deployConfigTargets).not.toHaveBeenCalled();
  });

  it("已有任务运行时拒绝并发启动", async () => {
    useApp.setState({ livePages: { recordId: "p1", lines: [], progress: 0, progressMessage: "", status: "running", record: null } });
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

  // 快照 / 恢复 / 按配置三条发起路径共用 store.ts 的 launchContainer，这里只测一条即覆盖三者。
  it("按已保存配置发起容器备份：配置 id 原样提交，记录 id 回填到 liveContainer", async () => {
    const id = await useApp.getState().startContainerConfigBackup("cfg-1");
    expect(api.startContainerConfigBackup).toHaveBeenCalledWith("cfg-1");
    expect(id).toBe("c3");
    expect(useApp.getState().liveContainer).toEqual({
      recordId: "c3",
      lines: [],
      progress: 0,
      progressMessage: "",
      status: "running",
      record: null,
    });
  });

  it("已有容器任务运行时拒绝再次发起", async () => {
    useApp.setState({
      liveContainer: { recordId: "c1", lines: [], progress: 0, progressMessage: "", status: "running", record: null },
    });
    await expect(useApp.getState().startContainerTransfer(containerRequest())).rejects.toThrow();
    expect(api.startContainerTransfer).not.toHaveBeenCalled();
  });

  it("容器任务发起失败时收回占位，界面不会卡在运行中", async () => {
    vi.mocked(api.startContainerTransfer).mockRejectedValueOnce(new Error("已有容器任务正在进行"));
    await expect(useApp.getState().startContainerTransfer(containerRequest())).rejects.toThrow();
    expect(useApp.getState().liveContainer).toBeNull();
    expect(useApp.getState().toasts).toHaveLength(1);
  });
});

// 发起后中途断线不能抹掉已经出现的日志面板：本机失败是走事件收尾的、不抹，两边口径要一致。
describe("store 长任务发起失败时的面板收敛", () => {
  it("备份中途断线时保留已看到的日志，只把状态收成失败", async () => {
    vi.mocked(api.startBackup).mockImplementationOnce(async () => {
      useApp.getState().handleBackupEvent({ type: "log", level: "info", message: "正在下载导出包" });
      throw new Error("SSH 通道已关闭");
    });
    await expect(
      useApp.getState().startBackup({ serverId: "s1" }),
    ).rejects.toThrow();
    const live = useApp.getState().liveBackup;
    expect(live?.status).toBe("failed");
    expect(live?.lines.map((line) => line.message)).toEqual([
      "正在下载导出包",
      "Error: SSH 通道已关闭",
    ]);
  });

  it("容器任务断线后界面恢复可点：保留日志但状态不是 running", async () => {
    vi.mocked(api.startContainerConfigBackup).mockImplementationOnce(async () => {
      useApp
        .getState()
        .handleContainerEvent({ type: "log", level: "info", message: "正在打包卷" });
      throw new Error("任务异常结束");
    });
    await expect(useApp.getState().startContainerConfigBackup("cc1")).rejects.toThrow();
    expect(useApp.getState().liveContainer?.status).toBe("failed");

    // 没跑起来的那次仍然要把占位收回去，否则界面永远点不动。
    vi.mocked(api.startContainerConfigBackup).mockRejectedValueOnce(new Error("已有容器任务"));
    await expect(useApp.getState().startContainerConfigBackup("other")).rejects.toThrow();
    expect(useApp.getState().liveContainer).toBeNull();
  });
});
