export type DeployStatus = "running" | "success" | "failed";

export type LogLevel = "info" | "command" | "success" | "warn" | "error";

export type SshAuth =
  | { type: "password"; password: string }
  | { type: "privateKey"; keyPath: string; passphrase: string | null };

/** 备份任务的执行位（Rust: deploy_core::models::RunLocation）。 */
export type RunLocation = "local" | "remote";

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
    | /** 倒计时期间电脑睡眠（或时钟前跳），唤醒后计划已过期：刻意放弃这次关机，不是失败。 */
      "shutdownMissed"
    | "shutdownFailed";
  message?: string;
}

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
