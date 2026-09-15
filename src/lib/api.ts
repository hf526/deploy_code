import { invoke } from "@tauri-apps/api/core";

import type {
  BackupConfig,
  BackupRecord,
  BackupRequest,
  BackupTarget,
  Branch,
  Commit,
  DeployRecord,
  DeployRequest,
  EnvFileConfig,
  FileContent,
  FileEntry,
  GraphCommit,
  PagesConfig,
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
} from "./types";

/** 后端 Tauri 命令的类型化封装。 */
export const api = {
  // 应用
  getDataDir: () => invoke<string>("get_data_dir"),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<Settings>("save_settings", { settings }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => invoke<void>("set_autostart", { enabled }),

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

  // 部署
  startDeploy: (request: DeployRequest) => invoke<string>("start_deploy", { request }),
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
  clearHistory: () => invoke<void>("clear_history"),

  // 数据库备份
  startBackup: (request: BackupRequest) => invoke<string>("start_backup", { request }),
  listBackups: (serverId?: string | null) =>
    invoke<BackupRecord[]>("list_backups", { serverId: serverId ?? null }),
  getBackup: (backupId: string) => invoke<BackupRecord>("get_backup", { backupId }),
  deleteBackup: (backupId: string) => invoke<boolean>("delete_backup", { backupId }),
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
  clearPagesRecords: () => invoke<void>("clear_pages_records"),
  getPagesConfig: (repoId: string) => invoke<PagesConfig>("get_pages_config", { repoId }),
  savePagesConfig: (repoId: string, config: PagesConfig) =>
    invoke<PagesConfig>("save_pages_config", { repoId, config }),
  testPages: (repoId: string, config?: PagesConfig) =>
    invoke<string>("test_pages", { repoId, config }),
};
