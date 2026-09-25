import { describe, expect, it, vi } from "vitest";

// 文案函数依赖 i18n，测试统一用直返 key 的桩。
vi.mock("./i18n", () => ({ default: { t: (key: string) => key } }));

import {
  CRON_METHODS,
  CRON_PRESETS,
  cronMethodLabel,
  cronSummary,
  formatUnixSeconds,
  localTimeZone,
} from "./cronJob";

describe("cronMethodLabel", () => {
  it("下标与 cron-job.org 的 requestMethod 编号一致", () => {
    expect(CRON_METHODS).toHaveLength(9);
    expect(cronMethodLabel(0)).toBe("GET");
    expect(cronMethodLabel(1)).toBe("POST");
    expect(cronMethodLabel(8)).toBe("PATCH");
  });

  it("未知编号原样显示，不抛错", () => {
    expect(cronMethodLabel(200)).toBe("200");
  });
});

describe("cronSummary", () => {
  it("命中预设时给中文读法的 key", () => {
    expect(cronSummary("*/15 * * * *")).toBe("cronJobs.presetEvery15");
  });

  it("未命中时回落原始表达式，并折叠多余空白", () => {
    expect(cronSummary("0 3 * * 1,3,5")).toBe("0 3 * * 1,3,5");
    expect(cronSummary("  0   3  * * *  ")).toBe("0 3 * * *");
  });

  it("预设列表里的表达式都能被自己识别", () => {
    for (const preset of CRON_PRESETS) {
      expect(cronSummary(preset.cron)).toBe(preset.labelKey);
    }
  });
});

describe("formatUnixSeconds", () => {
  it("0 与非法值显示占位符", () => {
    expect(formatUnixSeconds(0)).toBe("-");
    expect(formatUnixSeconds(-1)).toBe("-");
  });

  it("正常时间戳输出 YYYY-MM-DD HH:mm（本地时区）", () => {
    expect(formatUnixSeconds(1_700_000_000)).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  });
});

describe("localTimeZone", () => {
  it("总能给出一个 IANA 时区名", () => {
    expect(localTimeZone().length).toBeGreaterThan(0);
  });
});
