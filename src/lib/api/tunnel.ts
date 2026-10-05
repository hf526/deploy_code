import { invoke } from "@tauri-apps/api/core";

import type {
  ServerConfig,
  ServerTunnelStatus,
  TunnelRule,
} from "../types";

export const tunnelCommands = {
  // SSH 隧道（本机端口 -> 服务器上的地址）
  saveServerTunnels: (serverId: string, tunnels: TunnelRule[]) =>
    invoke<ServerConfig>("save_server_tunnels", { serverId, tunnels }),
  listTunnelStatus: () => invoke<ServerTunnelStatus[]>("list_tunnel_status"),
  reconnectTunnels: (serverId: string) => invoke<void>("reconnect_tunnels", { serverId }),
};
