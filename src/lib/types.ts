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
  /** 目标服务器 id 列表：一条配置可以带多台，顺序即批量部署的执行顺序。 */
  serverIds: string[];
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
  /** cron-job.org 的 API Key（控制台生成）；未填写时定时请求页会引导去设置。 */
  cronjobApiKey: string;
  pagesHistoryLimit: number;
  /** 容器备份/迁移记录的保留条数。 */
  containerHistoryLimit: number;
  /** 单次容器快照 / 迁移的超时秒数（含两台服务器之间的中转）。 */
  containerTimeoutSecs: number;
  language: string;
  atomicRelease: boolean;
  releaseKeep: number;
  scheduledBackupEnabled: boolean;
  scheduledBackupTime: string;
  scheduledBackupConfigId: string | null;
  scheduledShutdownEnabled: boolean;
  scheduledShutdownTime: string;
}

/** 关机计划的来源：每天定时排的那一次，或用户手动按下的倒计时。 */
export type ShutdownSource = "scheduled" | "manual";

/** 已排定的关机计划（仍在可取消的倒计时阶段，尚未下发系统关机请求）。 */
export interface PendingShutdown {
  /** 下发系统关机请求的时刻（epoch 毫秒），与 Date.now() 同基准。 */
  atMs: number;
  source: ShutdownSource;
}

/** 定时关机状态（对应 src-tauri/src/commands/shutdown.rs）。 */
export interface ShutdownStatus {
  /** 待执行的关机计划；null 表示当前没有排定的关机。 */
  pending: PendingShutdown | null;
  /** 可取消窗口（秒）：剩余时间少于它时倒计时转为醒目样式。 */
  cancelWindowSecs: number;
}

/** 一类配置在导入时的去向计数。 */
export interface ImportCounts {
  added: number;
  overwritten: number;
}

/** 导入配置的比对结果。预览与实际导入共用同一套合并语义，因此两边数字必然一致。 */
export interface ImportPreview {
  exportedAt: string;
  servers: ImportCounts;
  repos: ImportCounts;
  backupTargets: ImportCounts;
  deployConfigs: ImportCounts;
  backupConfigs: ImportCounts;
  pagesConfigs: ImportCounts;
  /** 导出文件里被脱敏清空、因而保留本机值的敏感字段条数。 */
  keptLocalSecrets: number;
}

/** 定时任务通知（如定时备份启动/失败、定时关机已执行/失败）。 */
export interface SchedulerNotice {
  kind: "started" | "noConfig" | "failed" | "shutdownFired" | "shutdownFailed";
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
  /** 后端进度事件带的当前步骤文案（如「上传压缩包」），没有则为空串。 */
  progressMessage: string;
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
  /** 用列表里哪一条 Pages 配置部署；留空则沿用仓库绑定的默认配置。 */
  configId?: string | null;
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

// ---------------------------------------------------------------------------
// cron-job.org 云端定时请求（镜像 deploy-core src/cronjob.rs）
// ---------------------------------------------------------------------------

/** 请求头一条：UI 用有序列表编辑，提交时转成 header 字典。 */
export interface CronHeader {
  key: string;
  value: string;
}

/** cron-job.org 的调度结构：整数数组，`[-1]` 表示「任意」。 */
export interface CronSchedule {
  timezone: string;
  minutes: number[];
  hours: number[];
  mdays: number[];
  months: number[];
  wdays: number[];
}

export interface CronExtendedData {
  headers: Record<string, string>;
  body: string;
}

/** 远端任务（服务端为准，本地不落盘）。 */
export interface CronJob {
  jobId: number;
  title: string;
  url: string;
  enabled: boolean;
  /** 下标对应 CRON_METHODS：0=GET 1=POST 2=OPTIONS 3=HEAD 4=PUT 5=DELETE 6=TRACE 7=CONNECT 8=PATCH。 */
  requestMethod: number;
  requestTimeout: number;
  schedule: CronSchedule;
  /** 后端由 schedule 反算出的 5 段表达式，服务端并不返回它。 */
  cron: string;
  extendedData: CronExtendedData;
  lastDuration: number;
  /** 上次实际执行的 unix 秒；0 表示从未执行。 */
  lastExecution: number;
  nextExecution: number;
}

/** 新建 / 编辑任务的提交体，`cron` 为标准 5 段表达式。 */
export interface CronJobDraft {
  jobId: number | null;
  title: string;
  url: string;
  method: string;
  headers: CronHeader[];
  body: string;
  timeoutSecs: number;
  enabled: boolean;
  cron: string;
  timezone: string;
}

/** 一次执行记录。 */
export interface CronJobRun {
  /** 服务端给的执行标识，用作列表 key。 */
  identifier: string;
  date: number;
  status: number;
  statusText: string;
  httpStatus: number;
  duration: number;
  url: string;
}

// ---------------------------------------------------------------------------
// 容器备份与迁移（镜像 deploy-core src/container.rs）
// ---------------------------------------------------------------------------

/** 服务器上发现到的一个 docker-compose 项目。 */
export interface ComposeStack {
  /** compose 项目名（`-p` 参数，也是数据卷名前缀）。 */
  name: string;
  /** 服务数量。 */
  services: number;
  /** 当前处于运行中的容器数。 */
  running: number;
  /** compose 给出的状态摘要，如 `running (3)`。 */
  status: string;
  /** 项目目录下找到的 compose 配置文件（绝对路径）。 */
  files: string[];
  /** 项目工作目录。 */
  workingDir: string;
}

/** 项目挂载的一个数据卷。 */
export interface ComposeVolume {
  name: string;
  /** 宿主机上的卷目录（非 root 连接时可能读不到）。 */
  mountpoint: string;
  sizeBytes: number;
  /** 导出失败时用来挂载卷的镜像（取当前使用该卷的容器镜像，本机已有）。 */
  image: string;
  /** 宿主机路径可直接读取。 */
  readable: boolean;
}

/** 项目里的一个服务（按容器解析）。 */
export interface ComposeService {
  name: string;
  container: string;
  image: string;
  state: string;
  /** 已发布的端口映射，如 `0.0.0.0:80->80/tcp`。 */
  ports: string;
}

/** 快照前的项目详情与预检结果。 */
export interface ComposeStackDetail {
  stack: ComposeStack;
  services: ComposeService[];
  volumes: ComposeVolume[];
  /** 需要 `docker save` 的镜像（已去重）。 */
  images: string[];
  /** 项目目录下一起带走的 env 文件。 */
  envFiles: string[];
  /** 目标机上使用的 compose 命令（`docker compose` 或 `docker-compose`）。 */
  composeCommand: string;
  volumeBytes: number;
  imageBytes: number;
  /** 项目目录（compose 文件与 env）体积：打包时总会带上，不受卷/镜像开关影响。 */
  projectBytes: number;
  /** 预检提示（不阻断，界面上原样展示）。 */
  warnings: string[];
}

/** 迁移目标。 */
export interface ContainerTarget {
  serverId: string;
  /** 目标服务器上的项目目录。 */
  targetDir: string;
  /** 恢复后自动 `docker compose up -d`。 */
  startServices: boolean;
}

/** 发起一次容器快照 / 迁移的参数。 */
export interface ContainerRequest {
  serverId: string;
  project: string;
  /** 打包数据卷前先 `compose stop`、结束后 `compose start`，保证数据一致。 */
  pauseSource: boolean;
  includeVolumes: boolean;
  includeImages: boolean;
  /** 为空表示只备份到本机。 */
  target: ContainerTarget | null;
}

/** 从本机已有备份包再恢复一次到服务器。 */
export interface ContainerRestoreRequest {
  bundlePath: string;
  target: ContainerTarget;
}

export type ContainerRecordKind = "backup" | "migrate" | "restore";

/** 一次容器备份 / 迁移记录。 */
export interface ContainerRecord {
  id: string;
  kind: ContainerRecordKind;
  project: string;
  serverId: string;
  serverName: string;
  targetServerId: string;
  targetServerName: string;
  targetDir: string;
  /** 本机备份包路径；`restore` 记录里是被恢复的来源包。 */
  bundlePath: string;
  bundleSize: number;
  services: string[];
  volumes: string[];
  images: string[];
  includeVolumes: boolean;
  includeImages: boolean;
  status: DeployStatus;
  error: string | null;
  log: string;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number;
}

export type ContainerEvent =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "progress"; percent: number; message: string }
  | { type: "finished"; record: ContainerRecord };

export type LiveContainer = LiveTask<ContainerRecord>;
