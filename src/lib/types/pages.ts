import type { DeployStatus, LiveTask, LogLevel } from "./common";

export type PagesProvider = "cloudflare" | "github";

export interface PagesConfig {
  provider: PagesProvider;
  projectName: string;
  buildCommand: string;
  outputDir: string;
  branch: string;
  publishBranch: string;
}

/** 列表展示用的 Pages 配置条目（独立管理，可复用）。 */
export interface PagesConfigEntry {
  id: string;
  name: string;
  repoId: string;
  repoName: string;
  config: PagesConfig;
  createdAt: string;
}

export interface PagesDeployRecord {
  id: string;
  provider: PagesProvider;
  repoId: string;
  repoName: string;
  projectName: string;
  branch: string;
  commit: string;
  commitShort: string;
  status: DeployStatus;
  error: string | null;
  log: string;
  url: string | null;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number;
}

export interface PagesRequest {
  repoId: string;
  /** 用列表里哪一条 Pages 配置部署；留空则沿用仓库绑定的默认配置。 */
  configId?: string | null;
  provider?: PagesProvider | null;
  projectName?: string | null;
  buildCommand?: string | null;
  outputDir?: string | null;
  branch?: string | null;
  publishBranch?: string | null;
  skipBuild: boolean;
}

export type PagesEvent =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "finished"; record: PagesDeployRecord };

export type LivePages = LiveTask<PagesDeployRecord>;
