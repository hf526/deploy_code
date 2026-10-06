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
}
