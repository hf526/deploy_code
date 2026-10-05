import type { DeployStatus, LiveTask, LogLevel, RunLocation } from "./common";

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
