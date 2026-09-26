import { describe, expect, it } from "vitest";

import { pruneSelection, selectionState, toggleAll, toggleId } from "./selection";

const ids = (...values: string[]) => new Set(values);

describe("toggleId", () => {
  it("勾选与取消都返回新集合", () => {
    const current = ids("a");
    const added = toggleId(current, "b", true);
    expect([...added]).toEqual(["a", "b"]);
    expect(current.has("b")).toBe(false);
    expect([...toggleId(added, "a", false)]).toEqual(["b"]);
  });
});

describe("selectionState", () => {
  it("候选为空时不算半选", () => {
    expect(selectionState(ids("a"), [])).toBe("none");
  });

  it("按命中数量给出 none / some / all", () => {
    const candidates = ["a", "b", "c"];
    expect(selectionState(ids(), candidates)).toBe("none");
    expect(selectionState(ids("a"), candidates)).toBe("some");
    expect(selectionState(ids("a", "b", "c"), candidates)).toBe("all");
  });

  it("选中了候选之外的条目也不算全选", () => {
    expect(selectionState(ids("a", "gone"), ["a"])).toBe("all");
    expect(selectionState(ids("gone"), ["a", "b"])).toBe("none");
  });
});

describe("toggleAll", () => {
  it("未全选时补齐当前候选", () => {
    expect([...toggleAll(ids("a"), ["a", "b"])]).toEqual(["a", "b"]);
  });

  it("已全选时只取消这一批，保留其它选择", () => {
    const next = toggleAll(ids("a", "b", "z"), ["a", "b"]);
    expect([...next]).toEqual(["z"]);
  });
});

describe("pruneSelection", () => {
  it("丢掉列表里已经不存在的 id", () => {
    expect([...pruneSelection(ids("a", "gone"), ["a", "b"])]).toEqual(["a"]);
  });

  it("全部有效时内容不变", () => {
    expect([...pruneSelection(ids("a"), ["a"])]).toEqual(["a"]);
  });
});
