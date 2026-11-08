import { useTranslation } from "react-i18next";
import { Download, Upload, X } from "lucide-react";

import { Button } from "../../components/ui";
import { transferPercent, type TransferEntry } from "../../lib/transfer";
import type { TransferDirection } from "../../lib/types";
import { cn, humanSize } from "../../lib/utils";

/**
 * 终端区里的传输条带：ZMODEM 那句提示 + 正在进行的那几条进度。
 *
 * 它是这一页唯一说「文件正在走」的地方，所以刻意不做成 toast：一次大文件传几十分钟，
 * toast 三秒就没了，而用户要看的是「现在到哪儿了、怎么停」。
 */
export function TransferStrip({
  entries,
  zmodemHint,
  onPick,
  onCancel,
  onDismissZmodem,
}: {
  entries: TransferEntry[];
  /** 当前这个标签里的远端跑过 `rz` / `sz`。 */
  zmodemHint: boolean;
  onPick: (mode: TransferDirection) => void;
  onCancel: (id: string) => void;
  onDismissZmodem: () => void;
}) {
  const { t } = useTranslation();
  if (!zmodemHint && entries.length === 0) return null;

  return (
    <div className="flex shrink-0 flex-col">
      {zmodemHint && (
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-warn/25 bg-warn-soft px-3 py-1.5 text-[11.5px] text-warn">
          <span className="min-w-0 truncate">{t("transfer.zmodemTitle")}</span>
          <div className="flex shrink-0 items-center gap-1.5">
            <Button
              size="sm"
              variant="secondary"
              icon={<Upload className="size-3.5" />}
              onClick={() => onPick("upload")}
            >
              {t("transfer.uploadButton")}
            </Button>
            <Button
              size="sm"
              variant="secondary"
              icon={<Download className="size-3.5" />}
              onClick={() => onPick("download")}
            >
              {t("transfer.downloadButton")}
            </Button>
            <button
              type="button"
              className="grid size-5 shrink-0 place-items-center rounded text-warn transition-colors hover:bg-hover hover:text-ink"
              onClick={onDismissZmodem}
              title={t("transfer.dismiss")}
              aria-label={t("transfer.dismiss")}
            >
              <X className="size-3" />
            </button>
          </div>
        </div>
      )}

      {entries.map((entry) => {
        const percent = transferPercent(entry);
        const uploading = entry.direction === "upload";
        return (
          <div
            key={entry.id}
            // 远端路径挂在悬停上：那一串才是「落到哪儿了」，而行宽根本放不下它。
            title={entry.remotePath}
            className="flex shrink-0 items-center gap-2 border-b border-line bg-panel px-3 py-1.5 text-[11.5px] text-ink-dim"
          >
            {uploading ? (
              <Upload className="size-3.5 shrink-0 text-ink-faint" aria-hidden />
            ) : (
              <Download className="size-3.5 shrink-0 text-ink-faint" aria-hidden />
            )}
            <span className="min-w-0 max-w-56 truncate text-ink">{entry.fileName}</span>
            <div className="h-1 min-w-0 w-32 shrink-0 overflow-hidden rounded-full bg-line">
              {/* 总量还不知道时不给宽度只给个底色：0% 的说法是「刚开始」，而这里是「还不知道有多大」。 */}
              <div
                className={cn("h-full rounded-full bg-brand transition-all", percent === null && "opacity-40")}
                style={{ width: `${percent ?? 0}%` }}
              />
            </div>
            <span
              className="w-14 shrink-0 text-right font-mono text-[10.5px] tabular-nums"
              title={percent === null ? t("transfer.unknownTotal") : undefined}
            >
              {percent === null ? humanSize(entry.done) : `${percent}%`}
            </span>
            <button
              type="button"
              className="grid size-5 shrink-0 place-items-center rounded text-ink-faint transition-colors hover:bg-hover hover:text-ink"
              onClick={() => onCancel(entry.id)}
              title={t("transfer.cancel")}
              aria-label={t("transfer.cancel")}
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
