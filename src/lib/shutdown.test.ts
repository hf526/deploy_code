import { describe, expect, it } from "vitest";

import { formatClockTime, formatCountdown, isImminent, remainingSecs } from "./shutdown";
import type { PendingShutdown, ShutdownStatus } from "./types";

function plan(atMs: number, source: PendingShutdown["source"] = "manual"): PendingShutdown {
  return { atMs, source };
}

function status(
  pending: PendingShutdown | null,
  cancelWindowSecs = 60,
): ShutdownStatus {
  return { pending, cancelWindowSecs };
}

describe("remainingSecs", () => {
  it("没有排定关机时不显示倒计时", () => {
    expect(remainingSecs(null, 10_000)).toBeNull();
  });

  it("不足一秒向上取整，避免先显示 0 又跳回 1", () => {
    expect(remainingSecs(plan(11_500), 10_000)).toBe(2);
    expect(remainingSecs(plan(11_000), 10_000)).toBe(1);
  });

  it("已过点钳在 0，不出现负数", () => {
    expect(remainingSecs(plan(9_000), 10_000)).toBe(0);
  });
});

describe("isImminent", () => {
  it("剩余时间落在可取消窗口内才算醒目", () => {
    expect(isImminent(status(plan(65_000)), 5_000)).toBe(true);
    expect(isImminent(status(plan(65_001)), 5_000)).toBe(false);
    expect(isImminent(status(plan(65_000), 120), 5_000)).toBe(true);
  });

  it("没有计划时始终为 false", () => {
    expect(isImminent(status(null), 5_000)).toBe(false);
  });
});

describe("formatCountdown", () => {
  it("不足一小时用 mm:ss", () => {
    expect(formatCountdown(45)).toBe("00:45");
    expect(formatCountdown(60)).toBe("01:00");
    expect(formatCountdown(1830)).toBe("30:30");
  });

  it("超过一小时才加小时位", () => {
    expect(formatCountdown(3600)).toBe("1:00:00");
    expect(formatCountdown(3725)).toBe("1:02:05");
  });

  it("负数与小数不会写出难看的文案", () => {
    expect(formatCountdown(-5)).toBe("00:00");
    expect(formatCountdown(59.9)).toBe("00:59");
  });
});

describe("formatClockTime", () => {
  it("按本地时钟补零显示", () => {
    // 用本地时间构造，测试结果不受运行机器所在时区影响。
    expect(formatClockTime(new Date(2026, 0, 1, 4, 1).getTime())).toBe("04:01");
    expect(formatClockTime(new Date(2026, 0, 1, 23, 59).getTime())).toBe("23:59");
  });

  it("非法时间戳退回占位符", () => {
    expect(formatClockTime(Number.NaN)).toBe("-");
  });
});
