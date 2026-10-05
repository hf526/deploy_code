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
