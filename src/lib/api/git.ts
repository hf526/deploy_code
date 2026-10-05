import { invoke } from "@tauri-apps/api/core";

import type {
  Branch,
  Commit,
  FileContent,
  FileEntry,
  GraphCommit,
  ReplaceSummary,
  RepoStatus,
  ResolvedRev,
  SearchHit,
} from "../types";

export const gitCommands = {
  // Git
  watchRepo: (repoId: string, path: string) => invoke<void>("watch_repo", { repoId, path }),
  unwatchRepo: (repoId: string) => invoke<void>("unwatch_repo", { repoId }),
  listBranches: (repoId: string, includeRemote: boolean) =>
    invoke<Branch[]>("list_branches", { repoId, includeRemote }),
  checkoutBranch: (repoId: string, branch: string) =>
    invoke<string>("checkout_branch", { repoId, branch }),
  createBranch: (repoId: string, name: string, from: string | null, checkout: boolean) =>
    invoke<string>("create_branch", { repoId, name, from, checkout }),
  deleteBranch: (repoId: string, branch: string, force: boolean) =>
    invoke<string>("delete_branch", { repoId, branch, force }),
  repoLog: (repoId: string, limit: number) => invoke<Commit[]>("repo_log", { repoId, limit }),
  commitGraph: (repoId: string, head: string | null, limit: number) =>
    invoke<GraphCommit[]>("commit_graph", { repoId, head, limit }),
  repoStatus: (repoId: string) => invoke<RepoStatus>("repo_status", { repoId }),
  fileDiff: (repoId: string, path: string) => invoke<string>("file_diff", { repoId, path }),
  listDir: (repoId: string, path: string) => invoke<FileEntry[]>("list_dir", { repoId, path }),
  readFile: (repoId: string, path: string) =>
    invoke<FileContent>("read_repo_file", { repoId, path }),
  writeFile: (repoId: string, path: string, content: string) =>
    invoke<string>("write_repo_file", { repoId, path, content }),
  findFiles: (repoId: string, query: string) => invoke<string[]>("find_files", { repoId, query }),
  searchContent: (repoId: string, query: string, caseSensitive: boolean) =>
    invoke<SearchHit[]>("search_content", { repoId, query, caseSensitive }),
  replaceContent: (
    repoId: string,
    search: string,
    replacement: string,
    paths: string[],
    caseSensitive: boolean,
  ) => invoke<ReplaceSummary>("replace_content", { repoId, search, replacement, paths, caseSensitive }),
  commitChanges: (repoId: string, message: string, allowSensitive: boolean) =>
    invoke<string>("commit_changes", { repoId, message, allowSensitive }),
  sensitiveChanges: (repoId: string) => invoke<string[]>("sensitive_changes", { repoId }),
  resetHard: (repoId: string, rev: string) => invoke<string>("reset_hard", { repoId, rev }),
  fetchRepo: (repoId: string) => invoke<string>("fetch_repo", { repoId }),
  pullRepo: (repoId: string) => invoke<string>("pull_repo", { repoId }),
  pushRepo: (repoId: string) => invoke<string>("push_repo", { repoId }),
  resolveRev: (repoId: string, rev: string) => invoke<ResolvedRev>("resolve_rev", { repoId, rev }),
};
