import { Monitor, Moon, Sun } from "lucide-react";
import { useTranslation } from "react-i18next";

import { useTheme, type ThemeMode } from "../lib/theme";
import { cn } from "../lib/utils";

const OPTIONS: Array<{ mode: ThemeMode; labelKey: string; titleKey: string; Icon: typeof Sun }> = [
  { mode: "light", labelKey: "theme.light", titleKey: "theme.lightTitle", Icon: Sun },
  { mode: "dark", labelKey: "theme.dark", titleKey: "theme.darkTitle", Icon: Moon },
  { mode: "system", labelKey: "theme.system", titleKey: "theme.systemTitle", Icon: Monitor },
];

/** 三段式主题切换控件：浅色 / 深色 / 跟随系统。 */
export function ThemeToggle({ className }: { className?: string }) {
  const { t } = useTranslation();
  const [mode, setMode] = useTheme();
  return (
    <div
      className={cn(
        "flex items-center gap-0.5 rounded-md border border-line bg-field p-0.5",
        className,
      )}
      role="radiogroup"
      aria-label={t("theme.ariaLabel")}
    >
      {OPTIONS.map(({ mode: value, labelKey, titleKey, Icon }) => (
        <button
          key={value}
          type="button"
          role="radio"
          aria-checked={mode === value}
          title={t(titleKey)}
          onClick={() => setMode(value)}
          className={cn(
            "flex h-6 flex-1 items-center justify-center gap-1 rounded text-[11px] whitespace-nowrap transition-colors",
            mode === value ? "bg-chip text-ink" : "text-ink-faint hover:text-ink-dim",
          )}
        >
          <Icon className="size-3.5" />
          {t(labelKey)}
        </button>
      ))}
    </div>
  );
}
