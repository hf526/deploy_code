import { describe, expect, it } from "vitest";

import { COL_W, colX, laneColor, layoutGraph, PAD_L, ROW_H, rowY } from "./graph";
import type { GraphCommit } from "./types";

function commit(hash: string, parents: string[]): GraphCommit {
  return { hash, short: hash, parents, author: "tester", date: "2026-01-01", subject: hash, refs: [] };
}

describe("layoutGraph", () => {
  it("线性历史全部落在同一车道，相邻行连线不重复", () => {
    const layout = layoutGraph([
      commit("c", ["b"]),
      commit("b", ["a"]),
      commit("a", []),
    ]);
    expect(layout.nodes.map((node) => node.col)).toEqual([0, 0, 0]);
    expect(layout.maxCol).toBe(0);
    expect(layout.segments).toEqual([
      { gap: 0, col: 0, toCol: 0, curve: false, color: laneColor(0) },
      { gap: 1, col: 0, toCol: 0, curve: false, color: laneColor(0) },
    ]);
    expect(layout.height).toBe(3 * ROW_H);
    expect(layout.width).toBe(PAD_L + COL_W + 6);
  });

  it("合并提交为第二个父提交新开车道，汇合后回收", () => {
    const layout = layoutGraph([
      commit("a", ["b", "c"]),
      commit("b", ["d"]),
      commit("c", ["d"]),
      commit("d", []),
    ]);
    expect(layout.nodes.map((node) => node.col)).toEqual([0, 0, 1, 0]);
    expect(layout.maxCol).toBe(1);
    // 合并边：从 (row0,col0) 弯到 (row2,col1)。
    expect(layout.segments).toContainEqual({
      gap: 0,
      col: 0,
      toCol: 1,
      curve: true,
      color: laneColor(0),
    });
    // 已汇合的第二父边沿 col1 垂直下行到父提交。
    expect(layout.segments).toContainEqual({
      gap: 1,
      col: 1,
      toCol: 1,
      curve: false,
      color: laneColor(1),
    });
    expect(layout.width).toBe(PAD_L + 2 * COL_W + 6);
    expect(layout.height).toBe(4 * ROW_H);
  });

  it("同一条边只生成一次（去重）", () => {
    const layout = layoutGraph([
      commit("a", ["b"]),
      commit("b", []),
    ]);
    const keys = layout.segments.map((segment) => `${segment.gap}|${segment.col}|${segment.toCol}`);
    expect(new Set(keys).size).toBe(keys.length);
  });
});

describe("坐标与配色", () => {
  it("列/行坐标按固定尺寸计算", () => {
    expect(colX(0)).toBe(PAD_L + COL_W / 2);
    expect(colX(2)).toBe(PAD_L + 2 * COL_W + COL_W / 2);
    expect(rowY(0)).toBe(ROW_H / 2);
    expect(rowY(3)).toBe(3 * ROW_H + ROW_H / 2);
  });

  it("车道颜色循环取用（含负数回绕）", () => {
    expect(laneColor(0)).toBe("var(--lane-1)");
    expect(laneColor(7)).toBe("var(--lane-8)");
    expect(laneColor(8)).toBe("var(--lane-1)");
    expect(laneColor(-1)).toBe("var(--lane-8)");
  });
});
