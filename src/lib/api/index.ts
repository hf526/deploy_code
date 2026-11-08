import { appCommands } from "./app";
import { reposCommands } from "./repos";
import { gitCommands } from "./git";
import { serversCommands } from "./servers";
import { hostCommands } from "./host";
import { nginxCommands } from "./nginx";
import { deployCommands } from "./deploy";
import { backupCommands } from "./backup";
import { pagesCommands } from "./pages";
import { cronjobCommands } from "./cronjob";
import { containerCommands } from "./container";
import { terminalCommands } from "./terminal";
import { transferCommands } from "./transfer";

/** 后端 Tauri 命令的类型化封装。 */
export const api = {
  ...appCommands,
  ...reposCommands,
  ...gitCommands,
  ...serversCommands,
  ...hostCommands,
  ...nginxCommands,
  ...deployCommands,
  ...backupCommands,
  ...pagesCommands,
  ...cronjobCommands,
  ...containerCommands,
  ...terminalCommands,
  ...transferCommands,
};
