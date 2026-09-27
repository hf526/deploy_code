import type { ServerTunnelStatus, TunnelRule } from "./types";

/** 隧道表单的草稿：端口用文本输入，校验前不先变成 NaN。 */
export interface TunnelDraft {
  localPort: string;
  remoteHost: string;
  remotePort: string;
}

/** 远端地址的默认值：绝大多数场景就是「服务器上那个只监听本机的容器端口」。 */
export const DEFAULT_TUNNEL_HOST = "127.0.0.1";

export function emptyTunnelDraft(): TunnelDraft {
  return { localPort: "", remoteHost: DEFAULT_TUNNEL_HOST, remotePort: "" };
}

function validPort(value: string): boolean {
  const port = Number(value);
  return Number.isInteger(port) && port > 0 && port <= 65535;
}

/**
 * 草稿转规则：id 留空由后端补齐（与前端列表里的 key 用下标代替）。
 * 返回的 errorKey 是 i18n key，由调用方翻译并带上端口号。
 */
export function draftToRule(
  draft: TunnelDraft,
  existing: TunnelRule[],
): { rule: TunnelRule | null; errorKey: string | null } {
  if (!validPort(draft.localPort) || !validPort(draft.remotePort)) {
    return { rule: null, errorKey: "servers.tunnelErrorPort" };
  }
  const remoteHost = draft.remoteHost.trim();
  if (!remoteHost) {
    return { rule: null, errorKey: "servers.tunnelErrorHost" };
  }
  const localPort = Number(draft.localPort);
  if (existing.some((rule) => rule.enabled && rule.localPort === localPort)) {
    return { rule: null, errorKey: "servers.tunnelErrorDuplicate" };
  }
  return {
    rule: { id: "", localPort, remoteHost, remotePort: Number(draft.remotePort), enabled: true },
    errorKey: null,
  };
}

/** 列表里一条规则的展示文本：`localhost:18080 → 127.0.0.1:8080`。 */
export function tunnelEndpoint(rule: TunnelRule): string {
  return `localhost:${rule.localPort} → ${rule.remoteHost}:${rule.remotePort}`;
}

export function activeTunnelCount(rules: TunnelRule[]): number {
  return rules.filter((rule) => rule.enabled).length;
}

export function findTunnelStatus(
  list: ServerTunnelStatus[],
  serverId: string,
): ServerTunnelStatus | undefined {
  return list.find((item) => item.serverId === serverId);
}

/**
 * 徽章颜色：没在跑（没启用规则或已被收掉）为灰，在线为绿，
 * 端口就绑不上是配置问题（红），连不上服务器只是等重连（琥珀）。
 */
export function tunnelTone(
  status: ServerTunnelStatus | null | undefined,
  enabledCount: number,
): "green" | "amber" | "red" | "gray" {
  if (enabledCount === 0 || !status) return "gray";
  if (status.connected) return "green";
  if (status.rules.some((rule) => !rule.bound)) return "red";
  return "amber";
}

/** 状态摘要：当前真正能转出去的端口条数（已绑定 + 会话在线）。 */
export function activeRuleCount(status: ServerTunnelStatus | null | undefined): number {
  if (!status) return 0;
  return status.rules.filter((rule) => rule.active).length;
}
