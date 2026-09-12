import { Monitor, Moon, Sun } from "lucide-react";

import { useTheme, type ThemeMode } from "../lib/theme";
import { cn } from "../lib/utils";

const OPTIONS: Array<{ mode: ThemeMode; text: string; title: string; Icon: typeof Sun }> = [
  { mode: "light", text: "浅色", title: "浅色主题", Icon: Sun },
  { mode: "dark", text: "深色", title: "深色主题", Icon: Moon },
  { mode: "system", text: "系统", title: "跟随系统", Icon: Monitor },
];

/** 三段式主题切换控件：浅色 / 深色 / 跟随系统。 */
export function ThemeToggle({ className }: { className?: string }) {
  const [mode, setMode] = useTheme();
  return (
    <div
      className={cn(
        "flex items-center gap-0.5 rounded-md border border-line bg-field p-0.5",
        className,
      )}
      role="radiogroup"
      aria-label="主题"
    >
      {OPTIONS.map(({ mode: value, text, title, Icon }) => (
        <button
          key={value}
          type="button"
          role="radio"
          aria-checked={mode === value}
          title={title}
          onClick={() => setMode(value)}
          className={cn(
            "flex h-6 flex-1 items-center justify-center gap-1 rounded text-[11px] whitespace-nowrap transition-colors",
            mode === value ? "bg-chip text-ink" : "text-ink-faint hover:text-ink-dim",
          )}
        >
          <Icon className="size-3.5" />
          {text}
        </button>
      ))}
    </div>
  );
}
