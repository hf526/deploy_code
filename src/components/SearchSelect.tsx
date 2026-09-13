import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Check, ChevronDown } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Input, inputClass } from "./ui";
import { cn } from "../lib/utils";

export interface SearchOption {
  value: string;
  label: string;
  hint?: string;
  group?: string;
  icon?: ReactNode;
}

/** 可搜索下拉框：点击展开，支持输入过滤、键盘上下选择与回车确认。 */
export function SearchSelect({
  value,
  onChange,
  options,
  placeholder,
  disabled = false,
  allowCustom = false,
  emptyText,
  className,
  panelClassName,
  dropUp = false,
}: {
  value: string;
  onChange: (value: string) => void;
  options: SearchOption[];
  placeholder?: string;
  disabled?: boolean;
  /** 允许自由输入（用于提交号 / 标签等非列表值）。 */
  allowCustom?: boolean;
  emptyText?: string;
  className?: string;
  panelClassName?: string;
  /** 面板向上弹出（适用于靠近容器底部的场景）。 */
  dropUp?: boolean;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const selected = options.find((option) => option.value === value);
  const label = selected?.label ?? value;

  const filtered = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return options;
    return options.filter(
      (option) =>
        option.value.toLowerCase().includes(keyword) ||
        option.label.toLowerCase().includes(keyword),
    );
  }, [options, query]);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
    const timer = window.setTimeout(() => inputRef.current?.focus(), 0);
    const onClick = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onClick);
    return () => {
      document.removeEventListener("mousedown", onClick);
      window.clearTimeout(timer);
    };
  }, [open]);

  useEffect(() => {
    setActive(0);
  }, [query]);

  function choose(next: string) {
    onChange(next);
    setOpen(false);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((index) =>
        filtered.length === 0 ? 0 : Math.min(index + 1, filtered.length - 1),
      );
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((index) => Math.max(index - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const keyword = query.trim();
      const exact = filtered.find(
        (option) => keyword !== "" && option.value.toLowerCase() === keyword.toLowerCase(),
      );
      // 优先精确匹配 → 再取高亮项 → 最后才用自由输入（提交号 / 新分支名）。
      if (exact) {
        choose(exact.value);
      } else if (filtered[active]) {
        choose(filtered[active].value);
      } else if (allowCustom && keyword) {
        choose(keyword);
      }
    } else if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
    }
  }

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        disabled={disabled}
        onClick={() => setOpen((value) => !value)}
        className={cn(
          inputClass,
          "flex items-center justify-between gap-1 pr-2 text-left",
          className,
        )}
      >
        <span className={cn("truncate", !label && "text-ink-faint")}>
          {label || placeholder || t("common.selectPlaceholder")}
        </span>
        <ChevronDown
          className={cn(
            "size-3.5 shrink-0 text-ink-dim transition-transform",
            open && "rotate-180",
          )}
        />
      </button>

      {open && (
        <div
          onClick={(event) => event.stopPropagation()}
          className={cn(
            "ui-pop absolute left-0 right-0 z-40 flex max-h-72 flex-col overflow-hidden",
            dropUp ? "bottom-full mb-1" : "top-full mt-1",
            panelClassName,
          )}
        >
          <div className="shrink-0 border-b border-line p-1.5">
            <Input
              ref={inputRef}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={handleKeyDown}
              placeholder={t("common.searchPlaceholder")}
              className="h-7! text-xs!"
            />
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-1.5">
            {filtered.length === 0 ? (
              <p className="px-2 py-1.5 text-xs text-ink-faint">
                {allowCustom && query.trim()
                  ? t("common.useCustomValue", { value: query.trim() })
                  : emptyText ?? t("common.noMatches")}
              </p>
            ) : (
              filtered.map((option, index) => (
                <div key={`${option.group ?? ""}:${option.value}`}>
                  {option.group && option.group !== filtered[index - 1]?.group && (
                    <p className="px-2 py-1 text-[10px] font-semibold tracking-wide text-ink-faint uppercase">
                      {option.group}
                    </p>
                  )}
                  <button
                    type="button"
                    onMouseEnter={() => setActive(index)}
                    onClick={() => choose(option.value)}
                    className={cn(
                      "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors",
                      index === active
                        ? "bg-hover text-ink"
                        : "text-ink-dim hover:bg-hover hover:text-ink",
                    )}
                  >
                    {option.icon}
                    <span className="min-w-0 flex-1 truncate">{option.label}</span>
                    {option.hint && (
                      <span className="shrink-0 text-[10px] text-ink-faint">{option.hint}</span>
                    )}
                    {option.value === value && <Check className="size-3.5 shrink-0 text-pos" />}
                  </button>
                </div>
              ))
            )}
          </div>
        </div>
      )}
    </div>
  );
}
