import { describe, expect, it } from "vitest";

import {
  TREE_WIDTH_DEFAULT,
  TREE_WIDTH_MAX,
  TREE_WIDTH_MIN,
  clampTreeWidth,
  readStoredTreeWidth,
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