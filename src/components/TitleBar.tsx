import { useEffect, useState, type ReactNode } from "react";
import { Copy, GitBranch, Minus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { cn } from "../lib/utils";

const appWindow = getCurrentWindow();

/** 无边框窗口的自绘标题栏，与侧栏 / 页头融为一体。 */
export function TitleBar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let disposed = false;
    void appWindow.isMaximized().then((value) => {
      if (!disposed) setMaximized(value);
    });
    const unlisten = appWindow.onResized(() => {
      void appWindow.isMaximized().then(setMaximized);
    });
    return () => {
      disposed = true;
      void unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <div
      data-tauri-drag-region
      className="ui-titlebar flex h-9 shrink-0 items-center"
      onContextMenu={(event) => event.preventDefault()}
    >
      <div data-tauri-drag-region className="flex min-w-0 flex-1 items-center gap-2.5 pl-3">
        <span className="ui-logo flex size-5 shrink-0 items-center justify-center rounded-md text-white">
          <GitBranch className="size-3" strokeWidth={2.25} />
        </span>
        <span data-tauri-drag-region className="truncate text-xs select-none">
          <span className="font-semibold tracking-tight text-ink">DeployCode</span>
          <span className="text-ink-faint"> · 分支部署工具</span>
        </span>
      </div>

      <CaptionButton title="最小化" onClick={() => void appWindow.minimize()}>
        <Minus className="size-3.5" strokeWidth={1.5} />
      </CaptionButton>
      <CaptionButton title={maximized ? "还原" : "最大化"} onClick={() => void appWindow.toggleMaximize()}>
        {maximized ? (
          <Copy className="size-3" strokeWidth={1.5} />
        ) : (
          <Square className="size-3" strokeWidth={1.5} />
        )}
      </CaptionButton>
      <CaptionButton title="关闭" danger onClick={() => void appWindow.close()}>
        <X className="size-3.5" strokeWidth={1.5} />
      </CaptionButton>
    </div>
  );
}

function CaptionButton({
  title,
  danger,
  onClick,
  children,
}: {
  title: string;
  danger?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      data-tauri-drag-region="false"
      className={cn(
        "ui-caption-btn flex h-full w-11 shrink-0 items-center justify-center",
        danger && "ui-caption-btn-close",
      )}
    >
      {children}
    </button>
  );
}
