/** 会话树宽度：本机视图偏好，与字号同口径存 localStorage，不进 settings。 */

export const TREE_WIDTH_DEFAULT = 288;
export const TREE_WIDTH_MIN = 220;
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