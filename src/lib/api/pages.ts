import { invoke } from "@tauri-apps/api/core";

import type {
  PagesConfig,
  PagesConfigEntry,
  PagesDeployRecord,
  PagesRequest,
} from "../types";

export const pagesCommands = {
  // Cloudflare Pages
  startPagesDeploy: (request: PagesRequest) => invoke<string>("start_pages_deploy", { request }),
  listPagesRecords: (repoId?: string | null) =>
    invoke<PagesDeployRecord[]>("list_pages_records", { repoId: repoId ?? null }),
  getPagesRecord: (recordId: string) => invoke<PagesDeployRecord>("get_pages_record", { recordId }),
  deletePagesRecord: (recordId: string) => invoke<boolean>("delete_pages_record", { recordId }),
  deletePagesRecords: (recordIds: string[]) =>
    invoke<number>("delete_pages_records", { recordIds }),
  clearPagesRecords: () => invoke<void>("clear_pages_records"),
  listPagesConfigs: () => invoke<PagesConfigEntry[]>("list_pages_configs"),
  // 一条 Pages 配置：id 留空表示新建，后端补齐 id / 创建时间并回传落盘结果。
  savePagesConfig: (entry: PagesConfigEntry) =>
    invoke<PagesConfigEntry>("save_pages_config", { entry }),
  deletePagesConfig: (id: string) => invoke<boolean>("delete_pages_config", { id }),
  getRepoDefaultPagesConfig: (repoId: string) =>
    invoke<PagesConfigEntry | null>("get_repo_default_pages_config", { repoId }),
  testPages: (repoId: string, config?: PagesConfig) =>
    invoke<string>("test_pages", { repoId, config }),
};
