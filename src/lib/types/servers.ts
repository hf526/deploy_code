import type { DbBackupSource } from "./backup";
import type { SshAuth } from "./common";
import type { TunnelRule } from "./tunnel";

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

/** 一台主机在采集那一刻的资源占用，镜像 deploy-core `metrics::HostMetrics`。 */
export interface HostMetrics {
  /** 整机 CPU 占用百分比（0-100，所有核心合起来算）。 */
  cpuPercent: number;
  cpuCores: number;
  memoryTotalBytes: number;
  memoryUsedBytes: number;
}

/** 逐台采集的结果：连不上的那台只影响它自己这一行。 */
export interface ServerMetrics {
  serverId: string;
  metrics: HostMetrics | null;
  error: string | null;
}
