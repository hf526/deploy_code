import { useCallback, useEffect, useRef, useState } from "react";

interface AutoRefreshOptions {
  /** 定时轮询间隔（毫秒）；0 表示不轮询，只在进入页面 / 窗口重新可见时刷新。 */
  intervalMs?: number;
  /** 两次刷新之间的最小间隔，防止 focus 与 visibilitychange 连续触发打爆后端。 */
  minGapMs?: number;
  enabled?: boolean;
}

interface AutoRefreshResult {
  /** 是否正在刷新，用于按钮 loading 态。 */
  refreshing: boolean;
  /** 手动触发一次刷新；force = true 时跳过最小间隔限制。 */
  refresh: (force?: boolean) => Promise<void>;
}

/**
 * 页面数据的自动刷新：进入页面拉一次，窗口重新获得焦点 / 页面重新可见时补一次，
 * 再按固定间隔轮询。页面不可见（切到别的窗口）时全部跳过，避免无谓的 git / SSH 开销。
 *
 * 刷新失败只静默结束（不向上抛、不打断页面）；需要提示的由传入的 action 自己决定。
 */
export function useAutoRefresh(
  action: () => Promise<void>,
  options: AutoRefreshOptions = {},
): AutoRefreshResult {
  const { intervalMs = 0, minGapMs = 2000, enabled = true } = options;
  const [refreshing, setRefreshing] = useState(false);
  const actionRef = useRef(action);
  actionRef.current = action;

  const runningRef = useRef(false);
  const lastRunRef = useRef(0);
  const aliveRef = useRef(true);

  const refresh = useCallback(
    async (force = false) => {
      if (runningRef.current) return;
      if (!force && Date.now() - lastRunRef.current < minGapMs) return;
      runningRef.current = true;
      lastRunRef.current = Date.now();
      setRefreshing(true);
      try {
        await actionRef.current();
      } catch {
        // 自动刷新失败不打断页面；silent 模式下 action 本身也不会弹 toast。
      } finally {
        runningRef.current = false;
        if (aliveRef.current) setRefreshing(false);
      }
    },
    [minGapMs],
  );

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  useEffect(() => {
    if (!enabled) return;
    // 进入页面先拉一次：App 启动时的 loadAll 之后，仓库可能已被外部改动。
    void refresh(true);

    const wake = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    document.addEventListener("visibilitychange", wake);
    window.addEventListener("focus", wake);
    const timer = intervalMs > 0 ? window.setInterval(wake, intervalMs) : 0;

    return () => {
      document.removeEventListener("visibilitychange", wake);
      window.removeEventListener("focus", wake);
      if (timer) window.clearInterval(timer);
    };
  }, [enabled, intervalMs, refresh]);

  return { refreshing, refresh };
}
