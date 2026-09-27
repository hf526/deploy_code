import { describe, expect, it } from "vitest";

import { diskLevel, splitByLocation, syncNeeded } from "./agent";
import type { AgentStatus, BackupConfig, ContainerConfig } from "./types";

function status(patch: Partial<AgentStatus> = {}): AgentStatus {
  return {
    proto: 1,
    version: "0.1.2",
    timezone: "+0800",
    localTime: "2026-09-27 03:00:00",
    dataDir: "/var/lib/deploycode",
    totalBytes: 1000,
    freeBytes: 600,
    floorBytes: 100,
    bundleBytes: 0,
    bundleCount: 0,
    servers: 2,
    backupConfigs: 1,
    containerConfigs: 1,
    backupEnabled: true,
    backupTime: "03:00",
    backupConfigName: "主库",
    backupLastRun: "2026-09-27",
    containerEnabled: false,
    containerTime: "03:30",
    containerQueue: 0,
    containerLastRun: "",
    ...patch,
  };
}

function backupConfig(patch: Partial<BackupConfig> = {}): BackupConfig {
  return {
    id: "b1",
    name: "主库",
    serverId: "s1",
    source: { mode: "docker", container: "db", database: "app", username: "u", password: "", schema: "public" },
    targetId: null,
    supabaseUrl: null,
    runLocation: "remote",
    ...patch,
  };
}

function containerConfig(patch: Partial<ContainerConfig> = {}): ContainerConfig {
  return {
    id: "c1",
    name: "博客",
    serverId: "s1",
    project: "blog",
    pauseSource: false,
    includeVolumes: true,
    includeImages: true,
    target: null,
    createdAt: "",
    runLocation: "remote",
    ...patch,
  };
}

describe("diskLevel", () => {
  it("只在真的低于水位线时报低", () => {
    expect(diskLevel(status({ freeBytes: 99 }))).toBe("low");
    expect(diskLevel(status({ freeBytes: 100 }))).toBe("tight");
    expect(diskLevel(status({ freeBytes: 199 }))).toBe("tight");
    expect(diskLevel(status({ freeBytes: 200 }))).toBe("ok");
  });

  it("拿不到磁盘信息时不编造告警", () => {
    // 平台不支持磁盘查询时 status 里三个字节数都是 0（agent 侧 disk::check 返回 None）。
    expect(diskLevel(status({ totalBytes: 0, freeBytes: 0, floorBytes: 0 }))).toBe("ok");
  });
});

describe("splitByLocation", () => {
  it("两条都数到、互不重叠", () => {
    const parts = splitByLocation([
      backupConfig({ id: "b1" }),
      backupConfig({ id: "b2", runLocation: "local" }),
      backupConfig({ id: "b3" }),
    ]);
    expect(parts.remote.map((item) => item.id)).toEqual(["b1", "b3"]);
    expect(parts.local.map((item) => item.id)).toEqual(["b2"]);
  });

  it("空列表不报错", () => {
    expect(splitByLocation([])).toEqual({ local: [], remote: [] });
  });
});

describe("syncNeeded", () => {
  const configs = {
    backup: [backupConfig(), backupConfig({ id: "b2", runLocation: "local" })],
    container: [containerConfig()],
    servers: 2,
  };

  it("还没读过状态时不催用户下发", () => {
    expect(syncNeeded(null, configs)).toBe(false);
  });

  it("控制机那份与本机一致时不必再下发", () => {
    // 远端执行位：数据库 1 条 + 容器 1 条，服务器 2 台，正等于 status 里的数。
    expect(syncNeeded(status(), configs)).toBe(false);
  });

  it("新把一条改成交控制机跑就算落后", () => {
    const changed = {
      ...configs,
      backup: [backupConfig(), backupConfig({ id: "b2", runLocation: "remote" })],
    };
    expect(syncNeeded(status(), changed)).toBe(true);
  });

  it("多加一台服务器也要重新下发", () => {
    expect(syncNeeded(status(), { ...configs, servers: 3 })).toBe(true);
  });
});
