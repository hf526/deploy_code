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
  /** 失败后生成的 AI 诊断结果；旧记录没有该字段。 */
  aiDiagnosis: AiDiagnosis | null;
}

/** AI 诊断的置信度。 */
export type AiConfidence = "high" | "medium" | "low";

/** 后端预置的只读诊断命令（枚举即白名单，模型只能挑选 id）。 */
export type DiagnosticCommandId =
  | "diskUsage"
  | "memory"
  | "failedServices"
  | "targetDir"
  | "docker";

/** 诊断结果里允许一键执行的动作。 */
export type AiFixAction =
  | { type: "redeploy" }
  | { type: "openTerminal" }
  | { type: "runDiagnostic"; command: DiagnosticCommandId };

/** 一次 AI 失败诊断的结果。 */
export interface AiDiagnosis {
  summary: string;
  likelyCause: string;
  confidence: AiConfidence;
  steps: string[];
  /** 后端已按白名单过滤；凭据类怀疑一律为空。 */
  actions: AiFixAction[];
  /** 参与分析的脱敏日志字节数。 */
  logBytes: number;
  credentialSuspect: boolean;
  credentialNote: string | null;
  model: string;
  analyzedAt: string;
}

/** 发送前预览：真正要发给模型的脱敏上下文。 */
export interface DiagnosisPreview {
  context: string;
  logBytes: number;
  totalBytes: number;
}

/** 一条只读诊断命令的执行结果。 */
export interface DiagnosticOutput {
  command: DiagnosticCommandId;
  code: number;
  output: string;
  truncated: boolean;
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
