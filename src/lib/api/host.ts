import { invoke } from "@tauri-apps/api/core";

import type { ServerConfig, ServerHostStatus, TunnelRule } from "../types";

export const hostCommands = {
  // 主机常驻连接（隧道规则 + 会话状态 + 资源占用）
  saveServerTunnels: (serverId: string, tunnels: TunnelRule[]) =>
    invoke<ServerConfig>("save_server_tunnels", { serverId, tunnels }),
  /** 纯内存读一次：会话状态、每条规则的监听情况、最近一次采集的占用。 */
  listHostStatus: () => invoke<ServerHostStatus[]>("list_host_status"),
  /** 重建这一台的会话（本机端口重绑 + 重新连接，一连上就补采一次占用）。 */
  reconnectHost: (serverId: string) => invoke<void>("reconnect_host", { serverId }),
};
