import { invoke } from "@tauri-apps/api/core";

import type {
  AgentBinaryInfo,
  AgentStaleness,
  AgentStatus,
  AgentSyncReport,
  BackupRecord,
  ContainerRecord,
} from "../types";

export const agentCommands = {
  /** 控制机（备份 agent）：安装 / 下发 / 状态 / 日志 / 回读 / 即时发起。 */
  /** 本机这份 agent 二进制从哪来（安装包内置 / 手动放置 / 没有），只读盘、不连服务器。 */
  agentBinaryInfo: () => invoke<AgentBinaryInfo>("agent_binary_info"),
  installAgent: (serverId: string) => invoke<AgentStatus>("install_agent", { serverId }),
  uninstallAgent: (serverId: string) => invoke<string>("uninstall_agent", { serverId }),
  /** 换控制机时没能收回的旧机器 id 列表（只读本机那份待办，不连服务器）。 */
  agentOrphans: () => invoke<string[]>("agent_orphans"),
  /** 控制机上那份定时 / 执行位与本机设置是否已经不一致（只读本机，不连服务器）。 */
  agentStaleness: () => invoke<AgentStaleness>("agent_staleness"),
  agentStatus: (serverId = "") => invoke<AgentStatus>("agent_status", { serverId }),
  agentSync: (serverId = "") => invoke<AgentSyncReport>("agent_sync", { serverId }),
  agentLogs: (lines = 200) => invoke<string[]>("agent_logs", { lines }),
  agentBackupRecords: (limit = 50) => invoke<BackupRecord[]>("agent_backup_records", { limit }),
  agentContainerRecords: (limit = 50) =>
    invoke<ContainerRecord[]>("agent_container_records", { limit }),
  /** 让控制机立刻跑一条数据库备份配置；命令会等任务结束，进度走 backup://event。 */
  startAgentBackup: (configId: string) => invoke<string>("start_agent_backup", { configId }),
  startAgentContainer: (configId: string) => invoke<string>("start_agent_container", { configId }),
  restoreAgentContainer: (
    recordId: string,
    targetServerId: string,
    targetDir: string,
    startServices: boolean,
  ) =>
    invoke<string>("restore_agent_container", {
      recordId,
      targetServerId,
      targetDir,
      startServices,
    }),
};
