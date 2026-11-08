import type { HostMetrics, ServerHostStatus } from "./types";

/**
 * 服务器占用的展示口径。
 *
 * 数字只有 `list_host_status` 那一份内存快照，这里不采集、不缓存、也不替断线的那台兜一个旧值：
 * 采不到就照实说采不到（`metricsState` 的 failed / pending 两条），否则一个停在原地的百分比
 * 会被读成「实时但没变化」。
 */

/** 越过这两条线就把占用条与百分比换成警示色。 */
export const CPU_WARN_PERCENT = 85;
export const MEMORY_WARN_PERCENT = 90;
/** 再往上就不只是「注意」了：条与数字一起变红。 */
export const USAGE_DANGER_PERCENT = 95;

export function memoryPercent(metrics: HostMetrics): number {
  if (metrics.memoryTotalBytes <= 0) return 0;
  return Math.round((metrics.memoryUsedBytes / metrics.memoryTotalBytes) * 100);
}

export type UsageTone = "brand" | "warn" | "neg";

/** 一条占用的配色：`warnAt` 之下正常，越过危险线红，中间琥珀。 */
export function usageTone(percent: number, warnAt: number): UsageTone {
  if (percent >= USAGE_DANGER_PERCENT) return "neg";
  if (percent >= warnAt) return "warn";
  return "brand";
}

export const usageToneClass: Record<UsageTone, string> = {
  brand: "bg-brand",
  warn: "bg-warn",
  neg: "bg-neg",
};

export const usageTextColor: Record<UsageTone, string> = {
  brand: "text-ink-dim",
  warn: "text-warn",
  neg: "text-neg",
};

/** 一台主机此刻的占用：有数字 / 明确采不到（带原因）/ 还没有。三态在界面各画各的。 */
export type MetricsView =
  | { kind: "values"; metrics: HostMetrics }
  | { kind: "failed"; reason: string }
  | { kind: "pending" };

export function metricsView(status: ServerHostStatus | undefined): MetricsView {
  if (!status) return { kind: "pending" };
  if (status.metrics) return { kind: "values", metrics: status.metrics };
  if (status.metricsError) return { kind: "failed", reason: status.metricsError };
  return { kind: "pending" };
}

export type HostLink = "online" | "retrying" | "down" | "unknown";

/**
 * 常驻会话的状态点。
 *
 * `reason` 直接取后端给的那句话（在线时是连接时刻），文案由调用方挂到悬停上；
 * 界面不自己猜「连不上」还是「正在重连」——retries 是后端数出来的。
 *
 * 刚登记、这一轮还没连过（没重试、没报错、也没连接时刻）算「还不知道」而不是「坏了」：
 * 红的含义是这一次确实没连上，而那种机器下一秒就可能自己接回来。
 */
export function hostLink(
  status: ServerHostStatus | undefined,
): { link: HostLink; reason: string | null } {
  if (!status) return { link: "unknown", reason: null };
  if (status.connected) return { link: "online", reason: status.connectedAt };
  if (status.retries > 0) return { link: "retrying", reason: status.lastError };
  if (!status.lastError && !status.connectedAt) return { link: "unknown", reason: null };
  return { link: "down", reason: status.lastError };
}

export const hostLinkClass: Record<HostLink, string> = {
  online: "bg-pos",
  retrying: "bg-warn",
  down: "bg-neg",
  unknown: "bg-line-strong",
};
