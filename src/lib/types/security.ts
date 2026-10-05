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
