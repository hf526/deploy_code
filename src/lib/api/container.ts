import { invoke } from "@tauri-apps/api/core";

import type {
  ComposeStack,
  ComposeStackDetail,
  ContainerConfig,
  ContainerRecord,
  ContainerRequest,
  ContainerRestoreRequest,
} from "../types";

export const containerCommands = {
  // 容器备份与迁移
  getContainerBackupDir: () => invoke<string>("get_container_backup_dir"),
  listComposeStacks: (serverId: string) => invoke<ComposeStack[]>("list_compose_stacks", { serverId }),
  inspectComposeStack: (serverId: string, project: string) =>
    invoke<ComposeStackDetail>("inspect_compose_stack", { serverId, project }),
  /** 快照（可选目标时再恢复到目标服务器）；返回记录 id，进度走 container://event。 */
  startContainerTransfer: (request: ContainerRequest) =>
    invoke<string>("start_container_transfer", { request }),
  /** 已保存的容器备份配置：列表 / 保存 / 删除 / 按配置发起。 */
  listContainerConfigs: () => invoke<ContainerConfig[]>("list_container_configs"),
  saveContainerConfig: (config: ContainerConfig) =>
    invoke<ContainerConfig>("save_container_config", { config }),
  deleteContainerConfig: (configId: string) =>
    invoke<boolean>("delete_container_config", { configId }),
  startContainerConfigBackup: (configId: string) =>
    invoke<string>("start_container_config_backup", { configId }),
  restoreContainerBundle: (request: ContainerRestoreRequest) =>
    invoke<string>("restore_container_bundle", { request }),
  cancelContainer: (recordId: string) => invoke<string>("cancel_container", { recordId }),
  listContainerRecords: () => invoke<ContainerRecord[]>("list_container_records"),
  deleteContainerRecord: (recordId: string) =>
    invoke<boolean>("delete_container_record", { recordId }),
  clearContainerRecords: () => invoke<void>("clear_container_records"),
};
