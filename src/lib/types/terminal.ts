/**
 * 交互式终端的界面模型，镜像 deploy-core `terminal::TerminalInfo` / `TerminalEvent`。
 *
 * 标签的显示顺序与「哪一个是当前的」在前端自己的状态里（`src/lib/terminal.ts`），
 * 后端只回答「哪些还活着」。
 */

/** 一个还活着的终端标签。 */
export interface TerminalInfo {
  id: string;
  serverId: string;
  serverName: string;
  /** 报给远端的窗口尺寸：远端 shell 按这个宽度折行。 */
  cols: number;
  rows: number;
}

/**
 * 终端事件（`terminal://event`）。
 *
 * `data.bytes` 是 base64：PTY 输出是任意字节，含控制序列与非法 UTF-8，
 * 只有按字节传才不会把 vim 的画面画花。
 */
export type TerminalEvent =
  | { type: "opened"; id: string; serverId: string; serverName: string }
  | { type: "data"; id: string; bytes: string }
  | { type: "closed"; id: string; reason: string | null };
