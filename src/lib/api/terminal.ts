import { invoke } from "@tauri-apps/api/core";

import type { TerminalInfo } from "../types";

export const terminalCommands = {
  // 交互式终端：每条都是常驻会话上多出来的一条 PTY 通道，不占任务名额
  openTerminal: (serverId: string, cols: number, rows: number) =>
    invoke<TerminalInfo>("open_terminal", { serverId, cols, rows }),
  /** 键盘输入与粘贴：文本原样送过去，远端收到的就是那几个字节。 */
  sendTerminalInput: (terminalId: string, data: string) =>
    invoke<void>("send_terminal_input", { terminalId, data }),
  /** 窗口尺寸变化要报给远端，否则 vim/top 按旧尺寸画。 */
  resizeTerminal: (terminalId: string, cols: number, rows: number) =>
    invoke<void>("resize_terminal", { terminalId, cols, rows }),
  closeTerminal: (terminalId: string) => invoke<void>("close_terminal", { terminalId }),
  /** 当前还活着的标签：界面重载后拿它对账。 */
  listTerminals: () => invoke<TerminalInfo[]>("list_terminals"),
};
