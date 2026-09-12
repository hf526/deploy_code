import { open } from "@tauri-apps/plugin-dialog";
import type { NavigateFunction } from "react-router-dom";

import { useApp } from "./store";

/** IDE 式打开仓库：选一个文件夹即完成添加并直接进入工作区。 */
export async function openRepoFolder(navigate: NavigateFunction): Promise<void> {
  try {
    const selected = await open({ directory: true, multiple: false, title: "打开 Git 仓库文件夹" });
    if (typeof selected !== "string") return;
    const { addRepoFromPath } = useApp.getState();
    const repo = await addRepoFromPath(selected);
    if (repo) navigate(`/repos/${repo.id}`);
  } catch (error) {
    useApp.getState().toast("error", String(error));
  }
}
