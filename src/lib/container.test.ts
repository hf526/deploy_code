import { describe, expect, it, vi } from "vitest";

// containerKindLabel 走 i18n，测试统一用直返 key 的桩。
vi.mock("./i18n", () => ({ default: { t: (key: string) => key } }));

import { containerKindLabel, containerSummary } from "./container";
import type { ContainerRecord } from "./types";

function record(patch: Partial<ContainerRecord> = {}): ContainerRecord {
  return {
    id: "c1",
    kind: "backup",
    project: "blog",
    serverId: "s1",
    serverName: "prod-a",
    targetServerId: "",
    targetServerName: "",
    targetDir: "",
    bundlePath: "",
    bundleSize: 0,
    services: [],
    volumes: [],
    images: [],
    includeVolumes: true,
    includeImages: true,
    status: "success",
    error: null,
    log: "",
    startedAt: "",
    finishedAt: null,
    durationMs: 0,
    ...patch,
  };
}

describe("containerSummary", () => {
  it("只备份到本机时不带目标", () => {
    expect(containerSummary(record())).toBe("blog · prod-a");
  });

  it("迁移把目标机器接在末尾", () => {
    expect(
      containerSummary(record({ targetServerId: "s2", targetServerName: "prod-b" })),
    ).toBe("blog · prod-a → prod-b");
  });

  it("服务器名称缺失时退回 id，不显示空白", () => {
    expect(containerSummary(record({ serverName: "" }))).toBe("blog · s1");
  });
});

describe("containerKindLabel", () => {
  it("三种任务类型各有文案", () => {
    expect(containerKindLabel("backup")).toBe("containers.kindBackup");
    expect(containerKindLabel("migrate")).toBe("containers.kindMigrate");
    expect(containerKindLabel("restore")).toBe("containers.kindRestore");
  });
});
