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
