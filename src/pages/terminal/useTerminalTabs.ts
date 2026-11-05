import { useCallback, useEffect, useRef, useState } from "react";

import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import {
  createDispatcher,
  decodeTerminalBytes,
  dropTab,
  markTabClosed,
  reconcileTabs,
  tabFromInfo,
  type TerminalTab,
} from "../../lib/terminal";
import { useTauriEvent } from "../../lib/useTauriEvent";
import { INITIAL_DIMS } from "./TerminalView";
import type { TerminalEvent } from "../../lib/types";

/**
 * 终端标签的状态：开、关、与后端对账，以及把输出送到对应的 xterm 实例。
 *
 * 输出不进 React 状态：一次 `cat` 就是几千个事件，走一遍 setState 只会把界面卡在重渲染上，
 * 而 xterm 自己就要原始字节。所以这里只存「有哪些标签、哪个还活着」，字节直接交给分发器。
 */
export function useTerminalTabs() {
  const [tabs, setTabs] = useState<TerminalTab[]>([]);
  const [activeId, setActiveId] = useState("");
  const [busy, setBusy] = useState(false);

  const toast = useApp((state) => state.toast);
  const dispatcherRef = useRef<ReturnType<typeof createDispatcher> | null>(null);
  if (!dispatcherRef.current) dispatcherRef.current = createDispatcher();
  const dispatcher = dispatcherRef.current;

  // 关掉标签后要把焦点交给旁边的一个，而那时 setState 还没落地，只能读自己维护的这份快照。
  const tabsRef = useRef<TerminalTab[]>([]);
  tabsRef.current = tabs;
  const mountedRef = useRef(true);

  const attach = useCallback(
    (id: string, sink: Parameters<typeof dispatcher.attach>[1]) => dispatcher.attach(id, sink),
    [dispatcher],
  );
  const detach = useCallback((id: string) => dispatcher.detach(id), [dispatcher]);

  const openFor = useCallback(
    async (serverId: string) => {
      setBusy(true);
      try {
        const info = await api.openTerminal(serverId, INITIAL_DIMS.cols, INITIAL_DIMS.rows);
        if (!mountedRef.current) {
          // 页面已经切走：这条通道没人接，留着就是界面上看不见、却占着一个名额的死标签。
          void api.closeTerminal(info.id).catch(() => undefined);
          return;
        }
        setTabs((prev) => (prev.some((tab) => tab.id === info.id) ? prev : [...prev, tabFromInfo(info)]));
        setActiveId(info.id);
      } catch (err) {
        toast("error", String(err));
      } finally {
        setBusy(false);
      }
    },
    [toast],
  );

  const closeTab = useCallback(
    async (id: string) => {
      const remaining = dropTab(tabsRef.current, id);
      setTabs(remaining);
      dispatcher.forget(id);
      setActiveId((current) => {
        if (current !== id) return current;
        const index = tabsRef.current.findIndex((tab) => tab.id === id);
        return remaining[Math.min(Math.max(index, 0), remaining.length - 1)]?.id ?? "";
      });
      // 后端关不掉也要把标签从界面上摘掉：留着只会让人以为还连着。
      await api.closeTerminal(id).catch(() => undefined);
    },
    [dispatcher],
  );

  useTauriEvent<TerminalEvent>("terminal://event", (event) => {
    switch (event.type) {
      case "data":
        dispatcher.push(event.id, decodeTerminalBytes(event.bytes));
        break;
      // 通道结束：标签留着（已经打出来的东西还能看、能复制），只是不再接受输入。
      case "closed":
        setTabs((prev) => markTabClosed(prev, event.id, event.reason));
        break;
      // openFor 已经把标签加进去了，这里只补「不是本界面发起的」那一种。
      case "opened":
        setTabs((prev) =>
          prev.some((tab) => tab.id === event.id)
            ? prev
            : [
                ...prev,
                {
                  id: event.id,
                  serverId: event.serverId,
                  serverName: event.serverName,
                  closed: false,
                  reason: null,
                },
              ],
        );
        break;
    }
  });

  // 进页面先跟后端对账：界面重载后本地是空的，但会话可能还开着（切到别的应用再回来就是这样）。
  useEffect(() => {
    mountedRef.current = true;
    let cancelled = false;
    void api
      .listTerminals()
      .then((remote) => {
        if (cancelled) return;
        // 加法合并写在 updater 里：这一句落地时用户可能已经自己开了标签，那份 prev 是当下的
        // 真列表，取 effect 关闭时的快照会把他刚开的抹掉。
        setTabs((prev) => reconcileTabs(prev, remote));
        // 只在还没选中任何东西时兜一个，不抢他已经看着的那一个。
        setActiveId((current) => current || (remote[remote.length - 1]?.id ?? ""));
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      mountedRef.current = false;
    };
  }, []);

  return { tabs, activeId, setActiveId, busy, openFor, closeTab, attach, detach };
}
