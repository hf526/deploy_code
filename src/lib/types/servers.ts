import type { DbBackupSource } from "./backup";
import type { SshAuth } from "./common";
import type { TunnelRule, TunnelRuleStatus } from "./tunnel";

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

/** 一台主机在最近一次采集那一刻的资源占用，镜像 deploy-core `metrics::HostMetrics`。 */
export interface HostMetrics {
  /** 整机 CPU 占用百分比（0-100，所有核心合起来算）。 */
  cpuPercent: number;
  cpuCores: number;
  memoryTotalBytes: number;
  memoryUsedBytes: number;
}

/**
 * 一台服务器的常驻连接状态，镜像 deploy-core `hostlink::ServerHostStatus`。
 *
 * 会话、隧道规则与资源占用是同一条连接上的三件事，所以由同一次内存读取返回，
 * 界面按秒轮询它也不产生任何 SSH 往返。
 */
export interface ServerHostStatus {
  serverId: string;
  serverName: string;
  /** SSH 会话是否在线。 */
  connected: boolean;
  /** 连续重连次数，连上即清零。 */
  retries: number;
  lastError: string | null;
  connectedAt: string | null;
  /** 本次在线期间转发过的连接数。 */
  forwarded: number;
  rules: TunnelRuleStatus[];
  /** 最近一次成功采集的占用；为 null 就是此刻没有数字，界面不许拿旧的顶上。 */
  metrics: HostMetrics | null;
  /** 采不到的原因，与 metrics 成对更新。 */
  metricsError: string | null;
  /** 这一组数字是哪一刻采的。 */
  sampledAt: string | null;
}
