import { describe, expect, it } from "vitest";

import { NAV_WIDTH_COLLAPSED, NAV_WIDTH_EXPANDED, readStoredNavCollapsed } from "./nav";

describe("侧栏收起状态", () => {
  it("只认盘上的 1，其余都当展开", () => {
    expect(readStoredNavCollapsed("1")).toBe(true);
    expect(readStoredNavCollapsed("0")).toBe(false);
    expect(readStoredNavCollapsed(null)).toBe(false);
    expect(readStoredNavCollapsed("")).toBe(false);
    expect(readStoredNavCollapsed("true")).toBe(false);
  });

  it("收起宽度小于展开宽度，且都为整数像素", () => {
    expect(NAV_WIDTH_COLLAPSED).toBeLessThan(NAV_WIDTH_EXPANDED);
    expect(Number.isInteger(NAV_WIDTH_COLLAPSED)).toBe(true);
    expect(Number.isInteger(NAV_WIDTH_EXPANDED)).toBe(true);
  });
});
