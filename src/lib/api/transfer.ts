import { invoke } from "@tauri-apps/api/core";

import type { TransferInfo } from "../types";

export const transferCommands = {
  // 文件传输：走那一台已经在线的常驻会话开一路 SFTP，不占任务名额、也不写记录
  /** 远端登录后落在的那个目录，给弹窗一个能改的起点；定不出来就直接报错，没有兜底值。 */
  remoteHome: (serverId: string) => invoke<string>("remote_home", { serverId }),
  /** 本机文件传到远端：remotePath 可以是目录（以 / 结尾）也可以是完整目标文件名。 */
  startUpload: (serverId: string, localPath: string, remotePath: string) =>
    invoke<TransferInfo>("start_upload", { serverId, localPath, remotePath }),
  /** 远端文件取回本机：remotePath 是一个文件的绝对路径，localPath 由「另存为」选出。 */
  startDownload: (serverId: string, remotePath: string, localPath: string) =>
    invoke<TransferInfo>("start_download", { serverId, remotePath, localPath }),
  /** 取消一次传输；远端临时名删不掉时这句会带着那个路径，必须原样给用户看到。 */
  cancelTransfer: (transferId: string) => invoke<void>("cancel_transfer", { transferId }),
  /** 还在进行的传输：界面重载后拿它对账，否则进度条凭空消失而远端还在收文件。 */
  listTransfers: () => invoke<TransferInfo[]>("list_transfers"),
};
