import { invoke } from "@tauri-apps/api/core";

import type {
  BackupConfig,
  BackupRecord,
  BackupRequest,
  BackupTarget,
  Branch,
  Commit,
  ComposeStack,
  ComposeStackDetail,
  ContainerConfig,
  ContainerRecord,
  ContainerRequest,
  ContainerRestoreRequest,
  CronJob,
  CronJobDraft,
  CronJobRun,
  DeployConfig,
  DeployRecord,
  DeployRequest,
  EnvFileConfig,
  FileContent,
  FileEntry,
  GraphCommit,
  ImportPreview,
  NginxConfigContent,
  NginxConfigFile,
  NginxContainerInfo,
  PagesConfig,
  PagesConfigEntry,
  PagesDeployRecord,
  PagesRequest,
  RepoDetail,
  RepoInfo,
  RepoStatus,
  ReplaceSummary,
  RemoteRelease,
  ResolvedRev,
  SearchHit,
  SecurityReport,
  ServerConfig,
  Settings,
  ShutdownStatus,
} from "./types";

/** 后端 Tauri 命令的类型化封装。 */
export const api = {
  // 应用
  getDataDir: () => invoke<string>("get_data_dir"),
  getAppVersion: () => invoke<string>("get_app_version"),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  exportConfig: () => invoke<string>("export_config"),
  previewConfigImport: (jsonStr: string) =>
    invoke<ImportPreview>("preview_config_import", { jsonStr }),
  importConfig: (jsonStr: string) => invoke<ImportPreview>("import_config", { jsonStr }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => invoke<void>("set_autostart", { enabled }),
  getShutdownStatus: () => invoke<ShutdownStatus>("get_shutdown_status"),
  scheduleShutdown: (minutes: number) =>
    invoke<ShutdownStatus>("schedule_shutdown", { minutes }),
  cancelShutdown: () => invoke<ShutdownStatus>("cancel_shutdown"),

  // 仓库
  listRepos: () => invoke<RepoInfo[]>("list_repos"),
  repoDetail: (repoId: string) => invoke<RepoDetail>("repo_detail", { repoId }),
  addRepo: (args: {
    path: string;
    name?: string | null;
    defaultServerId?: string | null;
    defaultTargetDir?: string | null;
  }) => invoke<RepoInfo>("add_repo", args),
  updateRepo: (args: {
    repoId: string;
    name?: string | null;
    path?: string | null;
    defaultServerId?: string | null;
    defaultTargetDir?: string | null;
  }) => invoke<RepoInfo>("update_repo", args),
  removeRepo: (repoId: string) => invoke<void>("remove_repo", { repoId }),
  cloneRepo: (args: { url: string; parentDir: string; name?: string | null }) =>
    invoke<RepoInfo>("clone_repo", args),
  setRepoRemote: (repoId: string, url: string) =>
    invoke<RepoInfo>("set_repo_remote", { repoId, url }),
  saveRepoEnvFiles: (repoId: string, envFiles: EnvFileConfig[]) =>
    invoke<RepoInfo>("save_repo_env_files", { repoId, envFiles }),

  // Git
  watchRepo: (repoId: string, path: string) => invoke<void>("watch_repo", { repoId, path }),
  unwatchRepo: (repoId: string) => invoke<void>("unwatch_repo", { repoId }),
  listBranches: (repoId: string, includeRemote: boolean) =>
    invoke<Branch[]>("list_branches", { repoId, includeRemote }),
  checkoutBranch: (repoId: string, branch: string) =>
    invoke<string>("checkout_branch", { repoId, branch }),
  createBranch: (repoId: string, name: string, from: string | null, checkout: boolean) =>
    invoke<string>("create_branch", { repoId, name, from, checkout }),
  deleteBranch: (repoId: string, branch: string, force: boolean) =>
    invoke<string>("delete_branch", { repoId, branch, force }),
  repoLog: (repoId: string, limit: number) => invoke<Commit[]>("repo_log", { repoId, limit }),
  commitGraph: (repoId: string, head: string | null, limit: number) =>
    invoke<GraphCommit[]>("commit_graph", { repoId, head, limit }),
  repoStatus: (repoId: string) => invoke<RepoStatus>("repo_status", { repoId }),
  fileDiff: (repoId: string, path: string) => invoke<string>("file_diff", { repoId, path }),
  listDir: (repoId: string, path: string) => invoke<FileEntry[]>("list_dir", { repoId, path }),
  readFile: (repoId: string, path: string) =>
    invoke<FileContent>("read_repo_file", { repoId, path }),
  writeFile: (repoId: string, path: string, content: string) =>
    invoke<string>("write_repo_file", { repoId, path, content }),
  findFiles: (repoId: string, query: string) => invoke<string[]>("find_files", { repoId, query }),
  searchContent: (repoId: string, query: string, caseSensitive: boolean) =>
    invoke<SearchHit[]>("search_content", { repoId, query, caseSensitive }),
  replaceContent: (
    repoId: string,
    search: string,
    replacement: string,
    paths: string[],
    caseSensitive: boolean,
  ) => invoke<ReplaceSummary>("replace_content", { repoId, search, replacement, paths, caseSensitive }),
  commitChanges: (repoId: string, message: string, allowSensitive: boolean) =>
    invoke<string>("commit_changes", { repoId, message, allowSensitive }),
  sensitiveChanges: (repoId: string) => invoke<string[]>("sensitive_changes", { repoId }),
  resetHard: (repoId: string, rev: string) => invoke<string>("reset_hard", { repoId, rev }),
  fetchRepo: (repoId: string) => invoke<string>("fetch_repo", { repoId }),
  pullRepo: (repoId: string) => invoke<string>("pull_repo", { repoId }),
  pushRepo: (repoId: string) => invoke<string>("push_repo", { repoId }),
  resolveRev: (repoId: string, rev: string) => invoke<ResolvedRev>("resolve_rev", { repoId, rev }),

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
  enableServerGuard: (server: ServerConfig, threshold: number, windowMins: number) =>
    invoke<string>("enable_server_guard", { server, threshold, windowMins }),
  disableServerGuard: (server: ServerConfig) =>
    invoke<string>("disable_server_guard", { server }),

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
  listBackupConfigs: () => invoke<BackupConfig[]>("list_backup_configs"),
  saveBackupConfig: (config: BackupConfig) =>
    invoke<BackupConfig>("save_backup_config", { config }),
  deleteBackupConfig: (configId: string) =>
    invoke<boolean>("delete_backup_config", { configId }),

  // Cloudflare Pages
  startPagesDeploy: (request: PagesRequest) => invoke<string>("start_pages_deploy", { request }),
  listPagesRecords: (repoId?: string | null) =>
    invoke<PagesDeployRecord[]>("list_pages_records", { repoId: repoId ?? null }),
  getPagesRecord: (recordId: string) => invoke<PagesDeployRecord>("get_pages_record", { recordId }),
  deletePagesRecord: (recordId: string) => invoke<boolean>("delete_pages_record", { recordId }),
  deletePagesRecords: (recordIds: string[]) =>
    invoke<number>("delete_pages_records", { recordIds }),
  clearPagesRecords: () => invoke<void>("clear_pages_records"),
  listPagesConfigs: () => invoke<PagesConfigEntry[]>("list_pages_configs"),
  // 一条 Pages 配置：id 留空表示新建，后端补齐 id / 创建时间并回传落盘结果。
  savePagesConfig: (entry: PagesConfigEntry) =>
    invoke<PagesConfigEntry>("save_pages_config", { entry }),
  deletePagesConfig: (id: string) => invoke<boolean>("delete_pages_config", { id }),
  getRepoDefaultPagesConfig: (repoId: string) =>
    invoke<PagesConfigEntry | null>("get_repo_default_pages_config", { repoId }),
  testPages: (repoId: string, config?: PagesConfig) =>
    invoke<string>("test_pages", { repoId, config }),

  // 定时请求（cron-job.org 云端调度，任务不落本地盘）
  listCronJobs: () => invoke<CronJob[]>("list_cron_jobs"),
  saveCronJob: (draft: CronJobDraft) => invoke<void>("save_cron_job", { draft }),
  deleteCronJob: (jobId: number) => invoke<void>("delete_cron_job", { jobId }),
  setCronJobEnabled: (jobId: number, enabled: boolean) =>
    invoke<void>("set_cron_job_enabled", { jobId, enabled }),
  cronJobHistory: (jobId: number) => invoke<CronJobRun[]>("cron_job_history", { jobId }),

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
