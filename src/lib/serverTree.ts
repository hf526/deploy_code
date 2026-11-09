/** 会话树宽度：本机视图偏好，与字号同口径存 localStorage，不进 settings。 */

export const TREE_WIDTH_DEFAULT = 240;
export const TREE_WIDTH_MIN = 200;
export const TREE_WIDTH_MAX = 480;
export const TREE_WIDTH_STORAGE_KEY = "deploycode:servers.treeWidth";

export function clampTreeWidth(value: number): number {
  if (!Number.isFinite(value)) return TREE_WIDTH_DEFAULT;
  return Math.min(TREE_WIDTH_MAX, Math.max(TREE_WIDTH_MIN, Math.round(value)));
}

export function readStoredTreeWidth(raw: string | null): number {
  if (!raw) return TREE_WIDTH_DEFAULT;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed <= 0) return TREE_WIDTH_DEFAULT;
  return clampTreeWidth(parsed);
}

export function writeStoredTreeWidth(value: number): void {
  try {
    localStorage.setItem(TREE_WIDTH_STORAGE_KEY, String(clampTreeWidth(value)));
  } catch {
    // 隐私模式 / WebView 禁写：宽度照样生效，只是下次进来回到默认。
  }
}

export function readStoredTreeWidthSafe(): number {
  try {
    return readStoredTreeWidth(localStorage.getItem(TREE_WIDTH_STORAGE_KEY));
  } catch {
    return TREE_WIDTH_DEFAULT;
  }
}

/**
 * 会话树密度：紧凑模式只留名称与地址，给服务器多、一屏看不下的时候用。
 *
 * 与宽度、字号同例——本机视图偏好，存 localStorage，不进 settings（不参与导出 / 导入）。
 */
export const TREE_COMPACT_STORAGE_KEY = "deploycode:servers.compact";

export function readStoredCompact(raw: string | null): boolean {
  return raw === "1";
}

export function readStoredCompactSafe(): boolean {
  try {
    return readStoredCompact(localStorage.getItem(TREE_COMPACT_STORAGE_KEY));
  } catch {
    return false;
  }
}

export function writeStoredCompact(value: boolean): void {
  try {
    localStorage.setItem(TREE_COMPACT_STORAGE_KEY, value ? "1" : "0");
  } catch {
    // WebView 禁写：本次照样生效，下次进来回到详细模式。
  }
}

/**
 * 方向键在列表里的下一次落点：两端夹住，不做环绕。
 *
 * `current` 为 -1（还没选）时向下从 0 开始、向上贴到最后一行；空列表返回 -1。
 */
export function stepIndex(current: number, delta: number, length: number): number {
  if (length <= 0) return -1;
  if (current < 0) return delta > 0 ? 0 : length - 1;
  return Math.min(length - 1, Math.max(0, current + delta));
}