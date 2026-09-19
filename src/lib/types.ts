export type DeployStatus = "running" | "success" | "failed";
export type LogLevel = "info" | "command" | "success" | "warn" | "error";

export type SshAuth =
  | { type: "password"; password: string }
  | { type: "privateKey"; keyPath: string; passphrase: string | null };

export interface DbBackupSource {
  mode: "docker" | "system";
  container: string;
  database: string;
  username: string;
  password: string;
  schema: string;
}

export interface BackupTarget {
  id: string;
  name: string;
  url: string;
}

export interface BackupConfig {
  id: string;
  name: string;
  serverId: string;
  source: DbBackupSource;
  targetId: string | null;
  supabaseUrl: string | null;
}

export interface NginxContainerInfo {
  name: string;
  image: string;
  ports: string;
  /** 名称或镜像包含 nginx，界面上优先展示。 */
  nginx: boolean;
}

export interface NginxConfigFile {
  name: string;
  size: number;
}

export interface NginxConfigContent {
  name: string;
  path: string;
  content: string;
}

export interface ServerConfig {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  auth: SshAuth;
  defaultTargetDir: string;
  dbBackup?: DbBackupSource | null;
  backupTargetId?: string | null;
  supabaseUrl?: string | null;
  createdAt: string;
}

export interface EnvFileConfig {
  localPath: string;
  remotePath: string;
}

export interface RepoInfo {
  id: string;
  name: string;
  path: string;
  pathExists: boolean;
  isRepo: boolean;
  currentBranch: string;
  remote: string | null;
  changeCount: number;
  defaultServerId: string | null;
  defaultTargetDir: string;
  defaultPagesConfigId: string | null;
  envFiles: EnvFileConfig[];
}

export interface Branch {
  name: string;
  isCurrent: boolean;
  isRemote: boolean;
  upstream: string | null;
  lastCommit: string;
  lastCommitSubject: string;
  lastCommitDate: string;
}

export interface Commit {
  hash: string;
  short: string;
  author: string;
  date: string;
  subject: string;
}

export interface GraphRef {
  name: string;
  kind: "local" | "remote" | "tag";
  isHead: boolean;
}

export interface GraphCommit {
  hash: string;
  short: string;
  parents: string[];
  author: string;
  date: string;
  subject: string;
  refs: GraphRef[];
}

export interface FileChange {
  code: string;
  status: string;
  path: string;
}

export interface FileEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
}

export interface FileContent {
  path: string;
  content: string;
  truncated: boolean;
}

export interface SearchHit {
  path: string;
  line: number;
  text: string;
}

export interface ReplaceSummary {
  filesReplaced: number;
  matchesReplaced: number;
}

export interface RepoStatus {
  branch: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  changes: FileChange[];
}

export interface ResolvedRev {
  rev: string;
  hash: string;
  short: string;
  subject: string;
  author: string;
  date: string;
}

export interface RemoteRelease {
  name: string;
  current: boolean;
  modified: string;
}

export interface RepoDetail {
  repo: RepoInfo;
  status: RepoStatus | null;
}

export interface DeployRecord {
  id: string;
  repoId: string;
  repoName: string;
  rev: string;
  branch: string;
  worktree: boolean;
  atomicRelease: boolean;
  releaseDir: string | null;
  commit: string;
  commitShort: string;
  commitSubject: string;
  serverId: string;
  serverName: string;
  targetDir: string;
  scriptDir: string;
  scripts: string[];
  runScripts: boolean;
  envFiles: EnvFileConfig[];
  status: DeployStatus;
  error: string | null;
  log: string;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number;
}

export interface DeployRequest {
  repoId: string;
  rev: string;
  serverId: string;
  targetDir: string;
  runScripts: boolean;
  scriptDir: string;
  scripts: string[];
  uploadEnv: boolean;
}

/** 保存的服务器部署配置（列表化管理，一键部署）。 */
export interface DeployConfig {
  id: string;
  name: string;
  repoId: string;
  serverId: string;
  targetDir: string;
  /** 部署版本（分支 / 标签 / 提交）；为空表示打包当前工作区。 */
  rev: string;
  runScripts: boolean;
  scriptDir: string;
  scripts: string[];
  uploadEnv: boolean;
  createdAt: string;
}

export interface Settings {
  scriptDir: string;
  runScripts: boolean;
  connectTimeoutSecs: number;
  scriptTimeoutSecs: number;
  keepRemoteArchive: boolean;
  historyLimit: number;
  supabaseUrl: string;
  defaultBackupTargetId: string | null;
  backupHistoryLimit: number;
  backupTimeoutSecs: number;
  cloudflareApiToken: string;
  cloudflareAccountId: string;
  /** 主密码哈希（用于校验用户输入的主密码）；未设置时为 null。 */
  masterPasswordHash: string | null;
  githubToken: string;
  pagesHistoryLimit: number;
  language: string;
  atomicRelease: boolean;
  releaseKeep: number;
  scheduledBackupEnabled: boolean;
  scheduledBackupTime: string;
  scheduledBackupConfigId: string | null;
}

/** 导入配置的统计信息。 */
export interface ImportSummary {
  serversImported: number;
  reposImported: number;
  backupTargetsImported: number;
  deployConfigsImported: number;
  backupConfigsImported: number;
  pagesConfigsImported: number;
}

/** 定时任务通知（如定时备份启动/失败）。 */
export interface SchedulerNotice {
  kind: "started" | "noConfig" | "failed";
  message?: string;
}

export type DeployEvent =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "progress"; percent: number; message: string }
  | { type: "finished"; record: DeployRecord };

export interface LogLine {
  level: LogLevel;
  message: string;
}

/** 长任务（部署 / 备份 / Pages）的实时状态：字段一致，仅任务记录类型不同。 */
export interface LiveTask<TRecord> {
  recordId: string;
  lines: LogLine[];
  progress: number;
  status: DeployStatus;
  record: TRecord | null;
}

export type LiveDeploy = LiveTask<DeployRecord>;

export interface BackupRecord {
  id: string;
  serverId: string;
  serverName: string;
  database: string;
  schema: string;
  targetName: string;
  target: string;
  status: DeployStatus;
  error: string | null;
  log: string;
  dumpSize: number;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number;
}

export interface BackupRequest {
  serverId: string;
  backupConfigId?: string | null;
  source?: DbBackupSource | null;
  targetId?: string | null;
  supabaseUrl?: string | null;
  database?: string | null;
  schema?: string | null;
}

export type BackupEvent =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "progress"; percent: number; message: string }
  | { type: "finished"; record: BackupRecord };

export type LiveBackup = LiveTask<BackupRecord>;

export type PagesProvider = "cloudflare" | "github";

export interface PagesConfig {
  provider: PagesProvider;
  projectName: string;
  buildCommand: string;
  outputDir: string;
  branch: string;
  publishBranch: string;
}

/** 列表展示用的 Pages 配置条目（独立管理，可复用）。 */
export interface PagesConfigEntry {
  id: string;
  name: string;
  repoId: string;
  repoName: string;
  config: PagesConfig;
  createdAt: string;
}

export interface PagesDeployRecord {
  id: string;
  provider: PagesProvider;
  repoId: string;
  repoName: string;
  projectName: string;
  branch: string;
  commit: string;
  commitShort: string;
  status: DeployStatus;
  error: string | null;
  log: string;
  url: string | null;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number;
}

export interface PagesRequest {
  repoId: string;
  provider?: PagesProvider | null;
  projectName?: string | null;
  buildCommand?: string | null;
  outputDir?: string | null;
  branch?: string | null;
  publishBranch?: string | null;
  skipBuild: boolean;
}

export type PagesEvent =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "finished"; record: PagesDeployRecord };

export type LivePages = LiveTask<PagesDeployRecord>;

export interface FailedLogin {
  user: string;
  ip: string;
  count: number;
}

export interface LoginEvent {
  user: string;
  ip: string;
  detail: string;
}

export interface OnlineSession {
  user: string;
  tty: string;
  loginAt: string;
  from: string;
}

export interface SecuritySetting {
  key: string;
  value: string;
}

export interface SecurityReport {
  isRoot: boolean;
  hasSudo: boolean;
  firewall: string;
  blocked: string[];
  guardEnabled: boolean;
  guardThreshold: number;
  guardWindowMins: number;
  failed: FailedLogin[];
  success: LoginEvent[];
  sessions: OnlineSession[];
  scannedAt: string;
  sshd: SecuritySetting[];
  notes: string[];
}
