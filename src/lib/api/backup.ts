import { invoke } from "@tauri-apps/api/core";

import type {
  BackupConfig,
  BackupRecord,
  BackupRequest,
  BackupTarget,
  OrphanBundle,
  OrphanCleanup,
} from "../types";

export const backupCommands = {
  // 数据库备份
  startBackup: (request: BackupRequest) => invoke<string>("start_backup", { request }),
  listBackups: (serverId?: string | null) =>
    invoke<BackupRecord[]>("list_backups", { serverId: serverId ?? null }),
  getBackup: (backupId: string) => invoke<BackupRecord>("get_backup", { backupId }),
  deleteBackup: (backupId: string) => invoke<boolean>("delete_backup", { backupId }),
  deleteBackups: (backupIds: string[]) => invoke<number>("delete_backups", { backupIds }),
  clearBackups: () => invoke<void>("clear_backups"),
  testBackup: (request: BackupRequest) => invoke<string>("test_backup", { request }),
  listBackupTargets: () => invoke<BackupTarget[]>("list_backup_targets"),
  saveBackupTargets: (targets: BackupTarget[]) =>
    invoke<BackupTarget[]>("save_backup_targets", { targets }),
  /** 备份包目录里没有记录指向的包（记录被裁剪或手动删掉之后留下的）。 */
  listOrphanBundles: () => invoke<OrphanBundle[]>("list_orphan_bundles"),
  deleteOrphanBundles: (paths: string[]) =>
    invoke<OrphanCleanup>("delete_orphan_bundles", { paths }),
  listBackupConfigs: () => invoke<BackupConfig[]>("list_backup_configs"),
  saveBackupConfig: (config: BackupConfig) =>
    invoke<BackupConfig>("save_backup_config", { config }),
  deleteBackupConfig: (configId: string) =>
    invoke<boolean>("delete_backup_config", { configId }),
};
