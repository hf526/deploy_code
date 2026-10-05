export interface EnvFileConfig {
  localPath: string;
  remotePath: string;
}

export interface RepoInfo {
  id: string;
  name: string;
  path: string;
  pathExists: boolean;
  isRepo: boolean;
  currentBranch: string;
  remote: string | null;
  changeCount: number;
  defaultServerId: string | null;
  defaultTargetDir: string;
  envFiles: EnvFileConfig[];
}

export interface Branch {
  name: string;
  isCurrent: boolean;
  isRemote: boolean;
  upstream: string | null;
  lastCommit: string;
  lastCommitSubject: string;
  lastCommitDate: string;
}

export interface Commit {
  hash: string;
  short: string;
  author: string;
  date: string;
  subject: string;
}

export interface GraphRef {
  name: string;
  kind: "local" | "remote" | "tag";
  isHead: boolean;
}

export interface GraphCommit {
  hash: string;
  short: string;
  parents: string[];
  author: string;
  date: string;
  subject: string;
  refs: GraphRef[];
}

export interface FileChange {
  code: string;
  status: string;
  path: string;
}

export interface FileEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
}

export interface FileContent {
  path: string;
  content: string;
  truncated: boolean;
}

export interface SearchHit {
  path: string;
  line: number;
  text: string;
}

export interface ReplaceSummary {
  filesReplaced: number;
  matchesReplaced: number;
}

export interface RepoStatus {
  branch: string;
  upstream: string | null;
  ahead: number;
  behind: number;
  changes: FileChange[];
}

export interface ResolvedRev {
  rev: string;
  hash: string;
  short: string;
  subject: string;
  author: string;
  date: string;
}

export interface RemoteRelease {
  name: string;
  current: boolean;
  modified: string;
}

export interface RepoDetail {
  repo: RepoInfo;
  status: RepoStatus | null;
}
