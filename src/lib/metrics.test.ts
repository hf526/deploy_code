import { describe, expect, it } from "vitest";

import {
  CPU_WARN_PERCENT,
  MEMORY_WARN_PERCENT,
  USAGE_DANGER_PERCENT,
  hostLink,
  memoryPercent,
  metricsView,
  usageTone,
} from "./metrics";
import type { HostMetrics, ServerHostStatus } from "./types";

function metrics(overrides: Partial<HostMetrics> = {}): HostMetrics {
  return {
    cpuPercent: 12,
    cpuCores: 4,
    memoryTotalBytes: 8_000_000_000,
    memoryUsedBytes: 2_000_000_000,
    ...overrides,
  };
}

function status(overrides: Partial<ServerHostStatus> = {}): ServerHostStatus {
  return {
    serverId: "s1",
    serverName: "深圳",
    connected: true,
    retries: 0,
    lastError: null,
    connectedAt: "2026-11-06 03:00:00",
    forwarded: 0,
    rules: [],
    metrics: metrics(),
    metricsError: null,
    sampledAt: "2026-11-06 03:00:04",
    ...overrides,
  };
}

describe("memoryPercent", () => {
  it("按 MemTotal 算使用比例", () => {
    expect(memoryPercent(metrics({ memoryTotalBytes: 4_000, memoryUsedBytes: 3_000 }))).toBe(75);
  });

  it("总量读成 0 时给 0 而不是除零", () => {
    expect(memoryPercent(metrics({ memoryTotalBytes: 0, memoryUsedBytes: 1_000 }))).toBe(0);
  });
});

describe("usageTone", () => {
  it("阈值之下是正常色", () => {
    expect(usageTone(CPU_WARN_PERCENT - 1, CPU_WARN_PERCENT)).toBe("brand");
    expect(usageTone(MEMORY_WARN_PERCENT - 1, MEMORY_WARN_PERCENT)).toBe("brand");
  });

  it("越过警示线换琥珀，再越过危险线换红", () => {
    expect(usageTone(CPU_WARN_PERCENT, CPU_WARN_PERCENT)).toBe("warn");
    expect(usageTone(USAGE_DANGER_PERCENT - 1, CPU_WARN_PERCENT)).toBe("warn");
    expect(usageTone(USAGE_DANGER_PERCENT, CPU_WARN_PERCENT)).toBe("neg");
    expect(usageTone(100, MEMORY_WARN_PERCENT)).toBe("neg");
  });
});

describe("metricsView", () => {
  it("有数字就用数字", () => {
    const view = metricsView(status());
    expect(view.kind).toBe("values");
  });

  it("采不到就说采不到，并把原因带回去", () => {
    expect(metricsView(status({ metrics: null, metricsError: "连接已断开" }))).toEqual({
      kind: "failed",
      reason: "连接已断开",
    });
  });

  // 界面对「停在原地的旧数字」和「还没有数字」的说法不同，所以断线那台必须落到后两种之一。
  it("会话断了不会把上一次采集当成实时值", () => {
    const view = metricsView(status({ connected: false, metrics: null, metricsError: "超时" }));
    expect(view.kind).toBe("failed");
  });

  it("刚加进来、这一轮还没采完是待采而不是失败", () => {
    expect(metricsView(status({ metrics: null, metricsError: null }))).toEqual({ kind: "pending" });
    expect(metricsView(undefined)).toEqual({ kind: "pending" });
  });
});

describe("hostLink", () => {
  it("在线时把连接时刻带回去当悬停说明", () => {
    expect(hostLink(status())).toEqual({ link: "online", reason: "2026-11-06 03:00:00" });
  });

  it("重连中用后端数出来的次数判定，界面不自己猜", () => {
    expect(hostLink(status({ connected: false, retries: 3, lastError: "超时" }))).toEqual({
      link: "retrying",
      reason: "超时",
    });
  });

  it("没连上也没重试过是断开", () => {
    expect(hostLink(status({ connected: false, retries: 0, lastError: "认证失败" }))).toEqual({
      link: "down",
      reason: "认证失败",
    });
  });

  // 刚登记的那台：没重试过、没报过错、也没连接时刻。红点是「这一次确实没连上」，它还没到那一步。
  it("还没开始连的机器不算断开", () => {
    expect(
      hostLink(status({ connected: false, retries: 0, lastError: null, connectedAt: null })),
    ).toEqual({ link: "unknown", reason: null });
  });

  it("报过错但还没重试过是断开", () => {
    expect(
      hostLink(status({ connected: false, retries: 0, lastError: "超时", connectedAt: null })),
    ).toEqual({ link: "down", reason: "超时" });
  });

  it("后端这轮没给这一台的状态时不谎报", () => {
    expect(hostLink(undefined)).toEqual({ link: "unknown", reason: null });
  });
});
