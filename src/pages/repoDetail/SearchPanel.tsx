import { useEffect, useMemo, useRef, useState } from "react";
import { File, Loader2, Replace, Search } from "lucide-react";
import { useTranslation } from "react-i18next";

import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { SearchHit } from "../../lib/types";
import { cn } from "../../lib/utils";
import { Button } from "../../components/ui";

export function SearchPanel({
  repoId,
  onOpenHit,
  onReplaced,
}: {
  repoId: string;
  onOpenHit: (path: string, line: number) => void;
  onReplaced: () => void;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  // 记录产生 hits 的查询条件，避免用旧命中配新关键词做替换。
  const [hitsKey, setHitsKey] = useState<{ query: string; caseSensitive: boolean } | null>(null);
  const [searching, setSearching] = useState(false);
  const [showReplace, setShowReplace] = useState(false);
  const [replacement, setReplacement] = useState("");
  const [replacing, setReplacing] = useState(false);
  const requestSeq = useRef(0);
  const queryRef = useRef(query);
  queryRef.current = query;
  const caseSensitiveRef = useRef(caseSensitive);
  caseSensitiveRef.current = caseSensitive;

  useEffect(() => {
    const trimmed = query.trim();
    const seq = ++requestSeq.current;
    if (!trimmed) {
      setHits(null);
      setHitsKey(null);
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearching(true);
    // 查询变化立即清空旧结果：防抖/搜索期间不能再对旧命中执行替换。
    setHits(null);
    setHitsKey(null);
    const timer = window.setTimeout(() => {
      void api
        .searchContent(repoId, trimmed, caseSensitive)
        .then((result) => {
          if (!cancelled && seq === requestSeq.current) {
            setHits(result);
            setHitsKey({ query: trimmed, caseSensitive });
          }
        })
        .catch((error) => {
          if (!cancelled && seq === requestSeq.current) {
            setHits([]);
            toast("error", String(error));
          }
        })
        .finally(() => {
          if (!cancelled && seq === requestSeq.current) setSearching(false);
        });
    }, 260);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query, caseSensitive, repoId, toast]);

  const grouped = useMemo(() => {
    const map = new Map<string, SearchHit[]>();
    for (const hit of hits ?? []) {
      const list = map.get(hit.path);
      if (list) list.push(hit);
      else map.set(hit.path, [hit]);
    }
    return Array.from(map.entries());
  }, [hits]);

  const matchCount = hits?.length ?? 0;
  const truncated = matchCount >= 800;
  const hitsStale =
    !hitsKey || hitsKey.query !== query.trim() || hitsKey.caseSensitive !== caseSensitive;

  async function handleReplace() {
    const trimmed = query.trim();
    const caseAtRequest = caseSensitive;
    if (!trimmed || hitsStale || !grouped.length || replacing) return;
    setReplacing(true);
    const seq = ++requestSeq.current;
    try {
      const summary = await api.replaceContent(
        repoId,
        trimmed,
        replacement,
        grouped.map(([path]) => path),
        caseAtRequest,
      );
      toast("success", t("repoDetail.replacedSummary", {
        matches: summary.matchesReplaced,
        files: summary.filesReplaced,
      }));
      onReplaced();
      const next = await api.searchContent(repoId, trimmed, caseAtRequest);
      // 替换期间查询被改动时，不能再用旧结果覆盖新查询的命中。
      if (
        seq === requestSeq.current &&
        queryRef.current.trim() === trimmed &&
        caseSensitiveRef.current === caseAtRequest
      ) {
        setHits(next);
        setHitsKey({ query: trimmed, caseSensitive: caseAtRequest });
      }
    } catch (error) {
      toast("error", String(error));
    } finally {
      setReplacing(false);
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-1.5 px-2.5">
        <Search className="size-3.5 shrink-0 text-ink-dim" />
        <span className="text-xs font-semibold text-ink">{t("repoDetail.search")}</span>
        <button
          type="button"
          onClick={() => setShowReplace((value) => !value)}
          className={cn(
            "ml-auto rounded p-0.5 transition-colors",
            showReplace ? "text-brand" : "text-ink-dim hover:text-ink",
          )}
          title={t("repoDetail.showReplace")}
        >
          <Replace className="size-3.5" />
        </button>
      </div>

      <div className="flex flex-col gap-2 px-2 pb-2">
        <div className="flex items-center gap-1.5">
          <input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t("repoDetail.searchAllPlaceholder")}
            className="ui-input h-7 min-w-0 flex-1 rounded-md px-2 text-xs text-ink placeholder:text-ink-faint"
          />
          <button
            type="button"
            onClick={() => setCaseSensitive((value) => !value)}
            className={cn(
              "h-7 w-7 shrink-0 rounded-md border text-[11px] font-semibold transition-colors",
              caseSensitive
                ? "border-brand-line bg-brand-soft text-brand"
                : "border-line text-ink-dim hover:text-ink",
            )}
            title={t("repoDetail.caseSensitive")}
          >
            Aa
          </button>
        </div>
        {showReplace && (
          <>
            <div className="flex items-center gap-1.5">
              <input
                value={replacement}
                onChange={(event) => setReplacement(event.target.value)}
                placeholder={t("repoDetail.replaceWithPlaceholder")}
                className="ui-input h-7 min-w-0 flex-1 rounded-md px-2 text-xs text-ink placeholder:text-ink-faint"
              />
              <Button
                size="sm"
                variant="secondary"
                className="h-7 shrink-0"
                loading={replacing}
                disabled={matchCount === 0 || searching || hitsStale}
                onClick={() => void handleReplace()}
              >
                {t("repoDetail.replaceAllButton")}
              </Button>
            </div>
            {truncated && (
              <p className="text-[10px] leading-relaxed text-warn">
                {t("repoDetail.replaceTruncatedHint")}
              </p>
            )}
          </>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto border-t border-line">
        {searching ? (
          <p className="flex items-center gap-2 px-3 py-3 text-xs text-ink-dim">
            <Loader2 className="size-3.5 animate-spin" /> {t("repoDetail.searching")}
          </p>
        ) : !query.trim() ? (
          <p className="px-3 py-3 text-xs text-ink-faint">{t("repoDetail.searchHint")}</p>
        ) : matchCount === 0 ? (
          <p className="px-3 py-3 text-xs text-ink-faint">{t("repoDetail.noSearchResults")}</p>
        ) : (
          <>
            <p className="px-3 py-1.5 text-[11px] text-ink-dim">
              {t("repoDetail.matchSummary", { matches: matchCount, files: grouped.length })}
              {truncated && t("repoDetail.resultsTruncated")}
            </p>
            {grouped.map(([path, list]) => (
              <div key={path} className="mb-1">
                <div className="flex items-center gap-1.5 px-3 py-1 text-[11px] text-ink-dim">
                  <File className="size-3 shrink-0 text-ink-dim" />
                  <span className="truncate" title={path}>
                    {path}
                  </span>
                  <span className="ml-auto shrink-0 rounded bg-hover px-1.5 text-[10px]">
                    {list.length}
                  </span>
                </div>
                <ul>
                  {list.slice(0, 60).map((hit) => (
                    <li key={`${hit.path}:${hit.line}`}>
                      <button
                        type="button"
                        onClick={() => onOpenHit(hit.path, hit.line)}
                        className="flex w-full items-baseline gap-2 py-0.5 pl-6 pr-2 text-left hover:bg-hover"
                        title={hit.text}
                      >
                        <span className="w-8 shrink-0 text-right font-mono text-[10px] text-ink-faint">
                          {hit.line}
                        </span>
                        <span className="truncate font-mono text-[11px] text-ink">
                          {hit.text}
                        </span>
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}
