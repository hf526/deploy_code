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
