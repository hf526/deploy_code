import type { GraphCommit } from "./types";

/** 单行像素高度（提交节点圆心间距）。 */
export const ROW_H = 30;
/** 单个车道列宽。 */
export const COL_W = 18;
/** 图形区域左侧留白。 */
export const PAD_L = 12;

/** 车道颜色：引用 index.css 中的 --lane-N 主题令牌，随主题切换。 */
const PALETTE = [
  "var(--lane-1)",
  "var(--lane-2)",
  "var(--lane-3)",
  "var(--lane-4)",
  "var(--lane-5)",
  "var(--lane-6)",
  "var(--lane-7)",
  "var(--lane-8)",
];

/** 车道颜色。 */
export function laneColor(col: number): string {
  return PALETTE[((col % PALETTE.length) + PALETTE.length) % PALETTE.length];
}

export interface GraphNode {
  commit: GraphCommit;
  row: number;
  col: number;
}

/** 两行之间的一段连线。curve=true 表示从 col 弯向 toCol。 */
export interface Segment {
  gap: number;
  col: number;
  toCol: number;
  curve: boolean;
  color: string;
}

export interface GraphLayout {
  nodes: GraphNode[];
  maxCol: number;
  segments: Segment[];
  /** 每条连线归属的提交（用于 hover 高亮，可选使用）。 */
  width: number;
  height: number;
}

/**
 * 计算提交图布局：给每个提交分配一列（车道），并把父子关系栅格化成可绘制的线段。
 * 输入必须按拓扑顺序排列（后端用 git log --topo-order 保证父提交出现在子提交之后）。
 */
export function layoutGraph(commits: GraphCommit[]): GraphLayout {
  const nodes: GraphNode[] = [];
  const hashToRow: Record<string, number> = {};
  const hashToCol: Record<string, number> = {};
  // 每条车道当前“等待”的父提交哈希，null 表示空闲。
  const waiting: Array<string | null> = [];

  const firstFreeLane = (): number => {
    const idx = waiting.indexOf(null);
    if (idx >= 0) return idx;
    waiting.push(null);
    return waiting.length - 1;
  };

  commits.forEach((commit, row) => {
    // 找出所有正在等待当前提交的车道，取最小下标作为当前提交所在列，其余视为汇入。
    const claimers: number[] = [];
    for (let i = 0; i < waiting.length; i++) {
      if (waiting[i] === commit.hash) claimers.push(i);
    }

    let col: number;
    if (claimers.length > 0) {
      col = claimers[0];
      for (let i = 1; i < claimers.length; i++) waiting[claimers[i]] = null;
    } else {
      col = firstFreeLane();
    }

    hashToRow[commit.hash] = row;
    hashToCol[commit.hash] = col;
    nodes.push({ commit, row, col });

    const parents = commit.parents;
    // 首个父提交延续当前车道。
    waiting[col] = parents.length > 0 ? parents[0] : null;

    // 其余父提交（合并/多父）各自占用一条车道去等待。
    for (let i = 1; i < parents.length; i++) {
      const p = parents[i];
      const existing = waiting.indexOf(p);
      if (existing >= 0) continue; // 已有车道在等它
      const lane = firstFreeLane();
      waiting[lane] = p;
    }
  });

  // 生成线段（去重）。
  const seen = new Set<string>();
  const segments: Segment[] = [];
  const pushSeg = (gap: number, col: number, toCol: number) => {
    const curve = col !== toCol;
    const key = `${gap}|${col}|${toCol}`;
    if (seen.has(key)) return;
    seen.add(key);
    segments.push({ gap, col, toCol, curve, color: laneColor(col) });
  };

  nodes.forEach((node) => {
    const { row, col, commit } = node;
    commit.parents.forEach((p) => {
      const pr = hashToRow[p];
      if (pr === undefined || pr <= row) return;
      const pc = hashToCol[p];
      // 从子提交出发：第一段弯入父提交所在车道，之后沿父车道垂直下行，
      // 避免合并的第二父边一直占用子提交车道而与第一父边重叠。
      pushSeg(row, col, pc);
      for (let g = row + 1; g < pr; g++) pushSeg(g, pc, pc);
    });
  });

  const maxCol = nodes.reduce((m, n) => Math.max(m, n.col), 0);
  const width = PAD_L + (maxCol + 1) * COL_W + 6;
  const height = nodes.length * ROW_H;
  return { nodes, maxCol, segments, width, height };
}

/** 圆心 x 坐标。 */
export function colX(col: number): number {
  return PAD_L + col * COL_W + COL_W / 2;
}

/** 第 row 行圆心的 y 坐标。 */
export function rowY(row: number): number {
  return row * ROW_H + ROW_H / 2;
}
