import type { PendingShutdown, ShutdownStatus } from "./types";

/**
 * 后端状态还没拿到时的兜底阈值，与 deploy-core 的 `CANCEL_WINDOW_SECS` 一致。
 * 只用于摘要文案里的秒数不缺字，真正的判断都用后端下发的值。
 */
export const FALLBACK_CANCEL_WINDOW_SECS = 60;

/**
 * 关机倒计时剩余秒数：向上取整，避免最后不足 1 秒时先显示 0 又跳回 1。
 * 没有排定关机时返回 null。
 */
export function remainingSecs(
  pending: PendingShutdown | null,
  nowMs: number,
): number | null {
  if (!pending) return null;
  return Math.max(0, Math.ceil((pending.atMs - nowMs) / 1000));
}

/** 剩余时间是否已进入醒目提示窗口（阈值由后端随状态下发，两边不各写一份）。 */
export function isImminent(status: ShutdownStatus, nowMs: number): boolean {
  const remaining = remainingSecs(status.pending, nowMs);
  return remaining !== null && remaining <= status.cancelWindowSecs;
}

/** 倒计时文案：不足一小时用 `mm:ss`，更长再加小时位，避免状态栏被撑开。 */
export function formatCountdown(totalSecs: number): string {
  const secs = Math.max(0, Math.floor(totalSecs));
  const pad = (value: number) => String(value).padStart(2, "0");
  const hours = Math.floor(secs / 3600);
  const minutes = Math.floor((secs % 3600) / 60);
  const seconds = secs % 60;
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${pad(minutes)}:${pad(seconds)}`;
}

/** 关机时刻的本地时钟显示（`HH:MM`），用于「04:01 执行」这类摘要。 */
export function formatClockTime(atMs: number): string {
  const date = new Date(atMs);
  if (Number.isNaN(date.getTime())) return "-";
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
