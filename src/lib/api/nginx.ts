import { invoke } from "@tauri-apps/api/core";

import type {
  NginxConfigContent,
  NginxConfigFile,
  NginxContainerInfo,
} from "../types";

export const nginxCommands = {
  // Nginx
  listNginxContainers: (serverId: string) =>
    invoke<NginxContainerInfo[]>("list_nginx_containers", { serverId }),
  listNginxConfigs: (serverId: string, container: string, dir: string) =>
    invoke<NginxConfigFile[]>("list_nginx_configs", { serverId, container, dir }),
  readNginxConfig: (serverId: string, container: string, dir: string, name: string) =>
    invoke<NginxConfigContent>("read_nginx_config", { serverId, container, dir, name }),
  saveNginxConfig: (args: {
    serverId: string;
    container: string;
    dir: string;
    name: string;
    content: string;
  }) => invoke<string>("save_nginx_config", args),
  deleteNginxConfig: (serverId: string, container: string, dir: string, name: string) =>
    invoke<string>("delete_nginx_config", { serverId, container, dir, name }),
  reloadNginx: (serverId: string, container: string) =>
    invoke<string>("reload_nginx", { serverId, container }),
};
