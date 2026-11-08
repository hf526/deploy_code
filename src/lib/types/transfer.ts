/**
 * 文件传输的界面模型，镜像 deploy-core `transfer::TransferInfo` / `TransferEvent`。
 *
 * 与长任务那四类（部署 / 备份 / Pages / 容器）不同，传输**不占任务名额、不写记录**：它只是
 * 常驻会话上多出来的一路 SFTP。所以这里不进 `applyTaskEvent`，进度也只活在内存里。
 */

/** 传输方向，镜像 `TransferDirection`。 */
export type TransferDirection = "upload" | "download";

/** 一次传输的登记信息。 */
export interface TransferInfo {
  id: string;
  serverId: string;
  serverName: string;
  direction: TransferDirection;
  /** 展示用的文件名：上传是本机那个文件的名，下载是远端路径的名。 */
  fileName: string;
  /** 最终落到远端的绝对路径（不是那个先占位的临时名）。 */
  remotePath: string;
}

/**
 * 传输事件（`transfer://event`）。
 *
 * 进度给的是 `done`/`total` 原值而不是百分比：字节数与百分比只在前端算一份，两边不会分叉。
 * `total` 在头一条回调之前是 0，界面要按「还不知道总量」画。
 */
export type TransferEvent =
  | {
      type: "started";
      id: string;
      serverId: string;
      serverName: string;
      direction: TransferDirection;
      fileName: string;
      remotePath: string;
    }
  | { type: "progress"; id: string; done: number; total: number }
  | { type: "finished"; id: string }
  | { type: "cancelled"; id: string }
  | { type: "failed"; id: string; reason: string };
