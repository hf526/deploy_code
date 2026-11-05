import { invoke } from "@tauri-apps/api/core";

import type {
  SecurityReport,
  ServerConfig,
} from "../types";

export const serversCommands = {
  // 服务器
  listServers: () => invoke<ServerConfig[]>("list_servers"),
  saveServer: (server: ServerConfig) => invoke<ServerConfig>("save_server", { server }),
  deleteServer: (serverId: string) => invoke<void>("delete_server", { serverId }),
  testServer: (server: ServerConfig) => invoke<string>("test_server", { server }),
  scanServerSecurity: (server: ServerConfig) =>
    invoke<SecurityReport>("scan_server_security", { server }),
  blockServerIp: (server: ServerConfig, ip: string) =>
    invoke<string>("block_server_ip", { server, ip }),
  unblockServerIp: (server: ServerConfig, ip: string) =>
    invoke<string>("unblock_server_ip", { server, ip }),
  kickServerSession: (server: ServerConfig, tty: string) =>
    invoke<string>("kick_server_session", { server, tty }),
  enableServerGuard: (
    server: ServerConfig,
    threshold: number,
    windowMins: number,
    whitelist: string[],
  ) => invoke<string>("enable_server_guard", { server, threshold, windowMins, whitelist }),
  disableServerGuard: (server: ServerConfig) =>
    invoke<string>("disable_server_guard", { server }),
};
