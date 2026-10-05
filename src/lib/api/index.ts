import { appCommands } from "./app";
import { reposCommands } from "./repos";
import { gitCommands } from "./git";
import { serversCommands } from "./servers";
import { tunnelCommands } from "./tunnel";
import { nginxCommands } from "./nginx";
import { deployCommands } from "./deploy";
import { backupCommands } from "./backup";
import { pagesCommands } from "./pages";
import { cronjobCommands } from "./cronjob";
import { containerCommands } from "./container";
import { agentCommands } from "./agent";

/** 后端 Tauri 命令的类型化封装。 */
export const api = {
  ...appCommands,
  ...reposCommands,
  ...gitCommands,
  ...serversCommands,
  ...tunnelCommands,
  ...nginxCommands,
  ...deployCommands,
  ...backupCommands,
  ...pagesCommands,
  ...cronjobCommands,
  ...containerCommands,
  ...agentCommands,
};
