import { describe, expect, it } from "vitest";

import {
  TREE_WIDTH_DEFAULT,
  TREE_WIDTH_MAX,
  TREE_WIDTH_MIN,
  clampTreeWidth,
  readStoredCompact,
  readStoredTreeWidth,
  stepIndex,
} from "./serverTree";

describe("会话树宽度", () => {
  it("限幅在两端夹住，不做环绕", () => {
    expect(clampTreeWidth(100)).toBe(TREE_WIDTH_MIN);
    expect(clampTreeWidth(9999)).toBe(TREE_WIDTH_MAX);
    expect(clampTreeWidth(300.4)).toBe(300);
    expect(clampTreeWidth(Number.NaN)).toBe(TREE_WIDTH_DEFAULT);
  });

  it("认不出的盘上值回落默认，越界的夹回端点", () => {
    expect(readStoredTreeWidth(null)).toBe(TREE_WIDTH_DEFAULT);
    expect(readStoredTreeWidth("")).toBe(TREE_WIDTH_DEFAULT);
    expect(readStoredTreeWidth("abc")).toBe(TREE_WIDTH_DEFAULT);
    expect(readStoredTreeWidth("0")).toBe(TREE_WIDTH_DEFAULT);
    expect(readStoredTreeWidth("-20")).toBe(TREE_WIDTH_DEFAULT);
    expect(readStoredTreeWidth("320")).toBe(320);
    expect(readStoredTreeWidth("9999")).toBe(TREE_WIDTH_MAX);
  });
});

describe("会话树密度", () => {
  it("只认盘上的 1，其余都当详细模式", () => {
    expect(readStoredCompact("1")).toBe(true);
    expect(readStoredCompact("0")).toBe(false);
    expect(readStoredCompact(null)).toBe(false);
    expect(readStoredCompact("")).toBe(false);
    expect(readStoredCompact("true")).toBe(false);
  });
});

describe("方向键落点", () => {
  it("两端夹住，不环绕；还没选时向下从第一行、向上到最后一行开始", () => {
    expect(stepIndex(0, 1, 4)).toBe(1);
    expect(stepIndex(3, 1, 4)).toBe(3);
    expect(stepIndex(0, -1, 4)).toBe(0);
    expect(stepIndex(-1, 1, 4)).toBe(0);
    expect(stepIndex(-1, -1, 4)).toBe(3);
    expect(stepIndex(-1, 1, 0)).toBe(-1);
  });
});