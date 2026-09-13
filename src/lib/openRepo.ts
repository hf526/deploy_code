import { open } from "@tauri-apps/plugin-dialog";
import type { NavigateFunction } from "react-router-dom";

import i18n from "./i18n";
import { useApp } from "./store";
import { runGuarded } from "./unsavedGuard";

/** IDE 式打开仓库：选一个文件夹即完成添加并直接进入工作区。 */
export async function openRepoFolder(navigate: NavigateFunction): Promise<void> {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      title: i18n.t("nav.openRepoDialogTitle"),
    });
    if (typeof selected !== "string") return;
    const { addRepoFromPath } = useApp.getState();
    const repo = await addRepoFromPath(selected);
    if (repo) runGuarded(() => navigate(`/repos/${repo.id}`));
  } catch (error) {
    useApp.getState().toast("error", String(error));
  }
}
