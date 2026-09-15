import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * 订阅 Tauri 事件，并在组件卸载时自动取消订阅。
 * 回调保存在 ref 中：调用方无需为稳定引用而 useCallback，回调变化也不会重复订阅。
 */
export function useTauriEvent<T>(eventName: string, handler: (payload: T) => void): void {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<T>(eventName, (event) => {
      // 取消订阅生效前仍可能收到事件：卸载后直接忽略，避免回调操作已卸载组件。
      if (!disposed) handlerRef.current(event.payload);
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [eventName]);
}
