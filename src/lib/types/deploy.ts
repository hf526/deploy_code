import type { DeployStatus, LiveTask, LogLevel } from "./common";
import type { EnvFileConfig } from "./repo";

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

export type DeployEvent =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "progress"; percent: number; message: string }
  | { type: "finished"; record: DeployRecord }
  /** 批量部署被中止：某台连准备都没过，剩下的机器不再部署。那台没有自己的记录。 */
  | { type: "batchAborted"; succeeded: number; total: number; reason: string };

export type LiveDeploy = LiveTask<DeployRecord>;
