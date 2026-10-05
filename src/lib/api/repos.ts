import { invoke } from "@tauri-apps/api/core";

import type {
  EnvFileConfig,
  RepoDetail,
  RepoInfo,
} from "../types";

export const reposCommands = {
  // 仓库
  listRepos: () => invoke<RepoInfo[]>("list_repos"),
  repoDetail: (repoId: string) => invoke<RepoDetail>("repo_detail", { repoId }),
  addRepo: (args: {
    path: string;
    name?: string | null;
    defaultServerId?: string | null;
    defaultTargetDir?: string | null;
  }) => invoke<RepoInfo>("add_repo", args),
  updateRepo: (args: {
    repoId: string;
    name?: string | null;
    path?: string | null;
    defaultServerId?: string | null;
    defaultTargetDir?: string | null;
  }) => invoke<RepoInfo>("update_repo", args),
  removeRepo: (repoId: string) => invoke<void>("remove_repo", { repoId }),
  cloneRepo: (args: { url: string; parentDir: string; name?: string | null }) =>
    invoke<RepoInfo>("clone_repo", args),
  setRepoRemote: (repoId: string, url: string) =>
    invoke<RepoInfo>("set_repo_remote", { repoId, url }),
  saveRepoEnvFiles: (repoId: string, envFiles: EnvFileConfig[]) =>
    invoke<RepoInfo>("save_repo_env_files", { repoId, envFiles }),
};
