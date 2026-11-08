import { useCallback, useEffect, useRef, useState } from "react";

import i18n from "../../lib/i18n";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import {
  applyTransferEvent,
  findEntry,
  mergeSeededEntries,
  type TransferEntry,
} from "../../lib/transfer";
import { useTauriEvent } from "../../lib/useTauriEvent";
import type { TerminalEvent, TransferEvent } from "../../lib/types";

/**
 * 文件传输（SFTP，走那一台已经在线的常驻会话）在前端的这一份状态。
 *
 * 刻意不进 `applyTaskEvent` / `live*`：传输不占任务名额、不写记录，也就没有「重载后拿持久化
 * 记录对账」那一步——对账用的是 `list_transfers()`，它给的才是当下真的还在跑的那几条。
 * 进度一秒好几条，所以归约交给 `lib/transfer.ts` 的纯函数：没有变化时它把原数组交回去，
 * 界面不会为了同一个百分比再走一遍 React。
 */
export function useTransfers() {
  const [entries, setEntries] = useState<TransferEntry[]>([]);
  // ref 只在下面那两处写入口同步推进，**不在渲染期拿 state 回写**：eager 写已经保证它永远
  // 是最新的那一份，而渲染期回写会在一次被打断的渲染里把它退回上一帧，正好丢掉刚到的事件。
  const entriesRef = useRef<TransferEntry[]>([]);
  // 收过尾的 id：挂载对账那份快照可能比 `finished` 晚落地，不许把已经结束的传输复活。
  const settledRef = useRef<Set<string>>(new Set());

  // 哪个标签里的远端真跑了 `rz` / `sz`：条带挂在那一个标签上，而不是全局说一句。
  const [zmodemIds, setZmodemIds] = useState<string[]>([]);
  // 用户说过「别再提」的标签：远端那个 `rz` 等不到对端会每 30 秒重发一次握手，后端跟着重发
  // 提示，不记住这一次关闭的话条带会在他关掉之后一遍遍自己冒回来。标签 id 不复用，留着无害。
  const dismissedZmodemRef = useRef<Set<string>>(new Set());

  const toast = useApp((state) => state.toast);

  /**
   * 唯一的写入口，ref 与 state 在这里一起推进：收尾提示要说文件名，而小文件的 `started` 与
   * `finished` 可能在同一次渲染之前一起到齐，只靠渲染回写的话 ref 还是上一帧的列表，那条
   * 提示就会被当成「不认识的 id」吞掉。
   */
  const apply = useCallback((event: TransferEvent): TransferEntry | undefined => {
    const known = findEntry(entriesRef.current, event.id);
    const next = applyTransferEvent(entriesRef.current, event);
    if (next !== entriesRef.current) {
      entriesRef.current = next;
      setEntries(next);
    }
    return known;
  }, []);

  const commit = useCallback(
    (event: TransferEvent) => {
      // 记在「认不认识这个 id」之前：对账快照晚落地时，收尾事件到的那一刻列表里还没有它，
      // 而正是这一条决定了那份快照不许把它再画回来。
      if (event.type === "finished" || event.type === "cancelled" || event.type === "failed") {
        settledRef.current.add(event.id);
      }
      const known = apply(event);
      // 认不出这个 id 就不弹提示：界面从没显示过它，冒出一句「某文件已传完」只会让人去猜是谁。
      if (!known) return;
      if (event.type === "finished") {
        toast("success", i18n.t("transfer.finished", { name: known.fileName }));
      } else if (event.type === "cancelled") {
        toast("info", i18n.t("transfer.cancelled", { name: known.fileName }));
      } else if (event.type === "failed") {
        toast(
          "error",
          i18n.t("transfer.failedToast", { name: known.fileName, reason: event.reason }),
        );
      }
    },
    [apply, toast],
  );

  useTauriEvent<TransferEvent>("transfer://event", (event) => {
    // started 不弹提示：条带本来就因此出现，再叠一句 toast 是重复信息。
    if (event.type === "started" || event.type === "progress") {
      apply(event);
      return;
    }
    commit(event);
  });

  // 远端起 ZMODEM 时后端只发提示、一个字节都不截：那条 `rz` 还在等它的协议对端，而我们
  // 不打算扮演，所以这一句只能指路（用上传 / 下载），不能说「已接管」。
  useTauriEvent<TerminalEvent>("terminal://event", (event) => {
    if (event.type !== "zmodemHint") return;
    if (dismissedZmodemRef.current.has(event.id)) return;
    // 已经标记过就返回原数组：同一个标签的 `rz` 会连着发好几条提示。
    setZmodemIds((prev) => (prev.includes(event.id) ? prev : [...prev, event.id]));
  });

  /** 用户点 × 或关掉那个标签：这条提示说过一次就够了，同样的画面不该再问第二次。 */
  const dismissZmodem = useCallback((terminalId: string) => {
    dismissedZmodemRef.current.add(terminalId);
    setZmodemIds((prev) =>
      prev.includes(terminalId) ? prev.filter((id) => id !== terminalId) : prev,
    );
  }, []);

  /**
   * 取消。后端那句「已取消，但远端临时文件 /root/.x.part 没能删掉」是这一句里唯一有用的
   * 信息（那个半截包得人工收），不能当成一次普通失败吞掉；成功那一路由 `cancelled` 事件
   * 定性与摘条带，这里不多嘴。
   */
  const cancel = useCallback(
    async (transferId: string) => {
      try {
        await api.cancelTransfer(transferId);
      } catch (error) {
        toast("error", String(error));
      }
    },
    [toast],
  );

  /** 发起成功返回 true（弹窗据此关闭）；失败只把后端那句中文弹出来，输入留着让用户改。 */
  const upload = useCallback(
    async (serverId: string, localPath: string, remotePath: string) => {
      try {
        const info = await api.startUpload(serverId, localPath, remotePath);
        commit({ type: "started", ...info });
        return true;
      } catch (error) {
        toast("error", String(error));
        return false;
      }
    },
    [commit, toast],
  );

  const download = useCallback(
    async (serverId: string, remotePath: string, localPath: string) => {
      try {
        const info = await api.startDownload(serverId, remotePath, localPath);
        commit({ type: "started", ...info });
        return true;
      } catch (error) {
        toast("error", String(error));
        return false;
      }
    },
    [commit, toast],
  );

  // 这一页是懒加载的：切走再回来本地列表是空的，而远端还在收文件。不对账的话进度条凭空
  // 消失，用户只能干等或者以为已经传完。
  useEffect(() => {
    let cancelled = false;
    void api
      .listTransfers()
      .then((infos) => {
        if (cancelled) return;
        // 只做加法：这份快照拍下之后用户可能已经自己发起了一条，取快照去 set 会把他刚发起
        // 的那条抹掉，而它后端还在跑。已经收过尾的那些则反过来——快照不许把它们复活。
        const next = mergeSeededEntries(entriesRef.current, infos, settledRef.current);
        if (next !== entriesRef.current) {
          entriesRef.current = next;
          setEntries(next);
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  return { entries, zmodemIds, dismissZmodem, cancel, upload, download };
}
