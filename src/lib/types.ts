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
  /** 执行位：本机定时，或交给控制机上的 agent（对应 Rust 的 RunLocation）。 */
  runLocation: RunLocation;
}

/** 备份任务的执行位（Rust: deploy_core::models::RunLocation）。 */
export type RunLocation = "local" | "remote";

/** 控制机 agent 回读的状态（Rust: deploy_core::agent::AgentStatus）。 */
export interface AgentStatus {
  proto: number;
  version: string;
  /** agent 主机的时区偏移，如 "+0800"。定时里的 HH:MM 按它解释。 */
  timezone: string;
  /** agent 主机的当前本地时间。 */
  localTime: string;
  dataDir: string;
  totalBytes: number;
  freeBytes: number;
  /** 水位线：低于这个字节数就拒绝新的备份任务。 */
  floorBytes: number;
  bundleBytes: number;
  bundleCount: number;
  servers: number;
  backupConfigs: number;
  containerConfigs: number;
  backupEnabled: boolean;
  backupTime: string;
  backupConfigName: string;
  backupLastRun: string;
  containerEnabled: boolean;
  containerTime: string;
  containerQueue: number;
  containerLastRun: string;
  /** 控制机上那份常驻 systemd 服务活着没有。由客户端问 systemd，不由 agent 自报；
   *  读不到（这台机器没有 systemctl）为 null。 */
  serviceActive: boolean | null;
}

/** 本机这份 agent 可执行文件的来源（Rust: deploy_core::agent::AgentBinarySource）。 */
export type AgentBinarySource = "bundled" | "manual" | "missing";

/** 安装包/数据目录里那份 agent 二进制的只读信息（Rust: deploy_core::agent::AgentBinaryInfo）。 */
export interface AgentBinaryInfo {
  source: AgentBinarySource;
  /** 绝对路径；source 为 "missing" 时是空串。 */
  path: string;
  sizeBytes: number;
  /** 用不了的原因（没内置、或放的那份不是 x86_64 Linux 可执行文件）；空串表示可用。 */
  error: string;
}

/** 盘上有文件、记录里已没有指向它的本地备份包（Rust: deploy_core::store::OrphanBundle）。 */
export interface OrphanBundle {
  /** "database" 走 backups 目录，"container" 走 containers 目录。 */
  kind: "database" | "container";
  fileName: string;
  path: string;
  sizeBytes: number;
}

/** 一次孤儿包清理的结果（Rust: commands::backup::OrphanCleanup）。 */
export interface OrphanCleanup {
  deleted: number;
  freedBytes: number;
}

/** 一次下发（注入）的结果（Rust: deploy_core::agent::AgentSyncReport）。 */
export interface AgentSyncReport {
  servers: number;
  backupConfigs: number;
  containerConfigs: number;
  warnings: string[];
  status: AgentStatus;
}

/** 控制机上那份与本机设置是否已经不一致（Rust: deploy_core::agent::AgentStaleness）。
 * 纯本机读盘：条数对得上时，界面也需要这一层来说出「那份是旧的」。 */
export interface AgentStaleness {
  /** 定时数据库备份（开关 / 时间 / 选中项）改动后没有重新下发。 */
  backupScheduleStale: boolean;
  /** 定时容器备份同上。 */
  containerScheduleStale: boolean;
  /** 执行位已改回本机、或配置已删除，但控制机那份里还留着它们（点名）。 */
  stragglers: string[];
  /** 上次成功下发的时刻；空串 = 从没下发过。 */
  syncedAt: string;
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
  /** 自动建立的 SSH 隧道规则（本机端口 → 该服务器上的地址）。 */
  tunnels: TunnelRule[];
  createdAt: string;
}

/** 一条 SSH 本地端口转发规则，等价 `ssh -L {localPort}:{remoteHost}:{remotePort}`。 */
export interface TunnelRule {
  id: string;
  localPort: number;
  /** 从服务器那一侧看的目标地址，容器一般写 127.0.0.1。 */
  remoteHost: string;
  remotePort: number;
  enabled: boolean;
}

export interface TunnelRuleStatus {
  ruleId: string;
  localPort: number;
  remoteLabel: string;
  /** 本机端口是否已绑定（绑定失败通常是端口被占用）。 */
  bound: boolean;
  /** 这条现在能不能真的转出去（已绑定 + SSH 在线）。 */
  active: boolean;
  error: string | null;
}

export interface ServerTunnelStatus {
  serverId: string;
  serverName: string;
  connected: boolean;
  /** 连续重连次数，连上即清零。 */
  retries: number;
  lastError: string | null;
  connectedAt: string | null;
  /** 本次在线期间转发过的连接数。 */
  forwarded: number;
  rules: TunnelRuleStatus[];
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
  /** 同一个 compose 项目在本机保留几个备份包，0 表示不自动清理。 */
  containerBundleKeep: number;
  /** 同一个（服务器 + 库 + schema）保留几个数据库导出包，0 表示不留。 */
  dbBundleKeep: number;
  language: string;
  atomicRelease: boolean;
  releaseKeep: number;
  scheduledBackupEnabled: boolean;
  scheduledBackupTime: string;
  scheduledBackupConfigId: string | null;
  /** 三条调度循环各自「已触发」的调度日期（YYYY-MM-DD，空表示从未）。 */
  scheduledBackupLastRun: string;
  scheduledShutdownEnabled: boolean;
  scheduledShutdownTime: string;
  scheduledShutdownLastRun: string;
  /** 容器定时备份：到点按 scheduledContainerConfigIds 的顺序逐个排队执行。 */
  scheduledContainerEnabled: boolean;
  scheduledContainerTime: string;
  scheduledContainerConfigIds: string[];
  scheduledContainerLastRun: string;
  /** 控制机（跑备份 agent 的那台服务器）的 id；空表示未启用。 */
  agentServerId: string;
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
  containerConfigs: ImportCounts;
  /** 导出文件里被脱敏清空、因而保留本机值的敏感字段条数。 */
  keptLocalSecrets: number;
}

/** 定时任务通知（定时备份 / 容器定时备份 / 定时关机的开始、跳过与失败）。 */
export interface SchedulerNotice {
  kind:
    | "started"
    | "noConfig"
    | "failed"
    | "containerStarted"
    | "containerNoConfig"
    | "containerFailed"
    | /** 到点了，但这条配置今晚由控制机执行：本机让位（正常收尾，不是失败）。 */
      "remoteSkipped"
    | /** 让位给控制机，可它拿的是上次下发的那份定时：今晚跑的不是用户以为的设置。 */
      "agentStale"
    | /** 执行位已改回本机的配置，控制机那份里还留着：今晚两边各跑一次。 */
      "agentStranded"
    | "shutdownFired"
    | "shutdownFailed";
  message?: string;
}

export type DeployEvent =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "progress"; percent: number; message: string }
  | { type: "finished"; record: DeployRecord }
  /** 批量部署被中止：某台连准备都没过，剩下的机器不再部署。那台没有自己的记录。 */
  | { type: "batchAborted"; succeeded: number; total: number; reason: string };

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
  /** 只有批量部署会有：中止那台没有记录，卡片靠这条说明「这批没跑完」。 */
  aborted?: { succeeded: number; total: number; reason: string } | null;
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
  /** 导出包在本机的路径；为空表示没留包（未开启留存，或已被轮转清掉）。 */
  bundlePath: string;
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
  /** 免封白名单（服务器上的 ALLOW= 配置，单个 IP）。 */
  whitelist: string[];
  /** 本次扫描这条 SSH 连接的来源 IP，未知时为空。 */
  selfIp: string;
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

/** 保存的容器备份配置：一次快照 / 迁移的全部参数，可复用、可定时。 */
export interface ContainerConfig {
  id: string;
  name: string;
  serverId: string;
  project: string;
  pauseSource: boolean;
  includeVolumes: boolean;
  includeImages: boolean;
  /** 迁移目标；null 表示只备份到本机。 */
  target: ContainerTarget | null;
  createdAt: string;
  /** 执行位：本机定时，或交给控制机上的 agent。 */
  runLocation: RunLocation;
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
