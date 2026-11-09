/** 左侧导航栏宽度：本机视图偏好，与字号 / 树宽同口径存 localStorage，不进 settings。 */

export const NAV_WIDTH_EXPANDED = 192;
export const NAV_WIDTH_COLLAPSED = 56;
export const NAV_COLLAPSED_STORAGE_KEY = "deploycode:nav.collapsed";

export function readStoredNavCollapsed(raw: string | null): boolean {
  return raw === "1";
}

export function readStoredNavCollapsedSafe(): boolean {
  try {
    return readStoredNavCollapsed(localStorage.getItem(NAV_COLLAPSED_STORAGE_KEY));
  } catch {
    return false;
  }
}

export function writeStoredNavCollapsed(value: boolean): void {
  try {
    localStorage.setItem(NAV_COLLAPSED_STORAGE_KEY, value ? "1" : "0");
  } catch {
    // WebView 禁写：本次照样生效，下次进来回到展开。
  }
}
