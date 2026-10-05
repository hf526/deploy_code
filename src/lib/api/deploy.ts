import { invoke } from "@tauri-apps/api/core";

import type {
  DeployConfig,
  DeployRecord,
  DeployRequest,
  RemoteRelease,
} from "../types";

export const deployCommands = {
  // 部署
  startDeploy: (request: DeployRequest) => invoke<string>("start_deploy", { request }),
  /** 按配置发起一批部署：serverIds 必须是配置里已登记的，返回本批覆盖的台数。 */
  deployConfigTargets: (configId: string, serverIds: string[]) =>
    invoke<number>("deploy_config_targets", { configId, serverIds }),
  listDeployConfigs: () => invoke<DeployConfig[]>("list_deploy_configs"),
  saveDeployConfig: (config: DeployConfig) =>
    invoke<DeployConfig>("save_deploy_config", { config }),
  deleteDeployConfig: (configId: string) =>
    invoke<boolean>("delete_deploy_config", { configId }),
  redeploy: (recordId: string) => invoke<string>("redeploy", { recordId }),
  cancelDeploy: (recordId: string) => invoke<string>("cancel_deploy", { recordId }),
  listReleases: (serverId: string, targetDir: string) =>
    invoke<RemoteRelease[]>("list_releases", { serverId, targetDir }),
  rollbackRelease: (serverId: string, targetDir: string, releaseName: string) =>
    invoke<string>("rollback_release", { serverId, targetDir, releaseName }),

  // 记录
  listHistory: (repoId?: string | null) =>
    invoke<DeployRecord[]>("list_history", { repoId: repoId ?? null }),
  getRecord: (recordId: string) => invoke<DeployRecord>("get_record", { recordId }),
  deleteRecord: (recordId: string) => invoke<boolean>("delete_record", { recordId }),
  deleteRecords: (recordIds: string[]) =>
    invoke<number>("delete_records", { recordIds }),
  clearHistory: () => invoke<void>("clear_history"),
};
