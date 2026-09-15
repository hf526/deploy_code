import { useEffect, useMemo, useRef, useState } from "react";
import {
  AlertTriangle,
  ChevronDown,
  ChevronUp,
  Columns2,
  File,
  FileDiff,
  Loader2,
  Replace,
  Save,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { DiffView } from "../../components/CodeView";
import { Button, EmptyState } from "../../components/ui";
import { highlightCode } from "../../lib/highlight";
import { cn } from "../../lib/utils";
import {
  EDITOR_PAD_TOP,
  findMatches,
  HEAVY_LIMIT,
  LINE_H,
  offsetToLine,
  replaceAt,
  type EditorView,
  type MatchRange,
  type Reveal,
} from "./editor";

export function EditorPane({
  editor,
  reveal,
  onClose,
  onDraftChange,
  onSave,
  onRevert,
  onReload,
  onKeepDraft,
  onRevealDone,
}: {
  editor: EditorView;
  reveal: Reveal | null;
  onClose: () => void;
  onDraftChange: (path: string, draft: string) => void;
  onSave: (path: string) => void;
  onRevert: (path: string) => void;
  onReload: (path: string) => void;
  onKeepDraft: (path: string) => void;
  onRevealDone: () => void;
}) {
  const { t } = useTranslation();
  if (editor.kind === "none") {
    return (
      <div className="flex flex-1 items-center justify-center p-8">
        <EmptyState
          icon={<Columns2 className="size-5" />}
          title={t("repoDetail.editorTitle")}
          description={t("repoDetail.editorDescription")}
        />
      </div>
    );
  }

  const isDiff = editor.kind === "diff";
  const isFile = editor.kind === "file";
  const dirty =
    isFile && editor.content !== null && editor.draft !== null && editor.draft !== editor.content;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* 标签页头 */}
      <div className="ui-titlebar flex h-9 shrink-0 items-center gap-2 border-b border-line pl-3">
        {isDiff ? (
          <FileDiff className="size-3.5 shrink-0 text-brand" />
        ) : (
          <File className="size-3.5 shrink-0 text-ink-dim" />
        )}
        <span className="truncate font-mono text-xs text-ink">{editor.path}</span>
        {dirty && (
          <span className="shrink-0 rounded-full bg-warn-soft px-1.5 text-[10px] text-warn">
            {t("repoDetail.unsaved")}
          </span>
        )}
        {editor.loading && <Loader2 className="size-3.5 shrink-0 animate-spin text-ink-dim" />}

        {isFile && !editor.loading && !editor.error && (
          <div className="ml-auto flex shrink-0 items-center gap-1.5">
            {dirty && (
              <Button size="sm" variant="ghost" onClick={() => onRevert(editor.path)}>
                {t("repoDetail.revert")}
              </Button>
            )}
            <Button
              size="sm"
              disabled={!dirty || editor.saving}
              loading={editor.saving}
              icon={<Save className="size-3.5" />}
              onClick={() => onSave(editor.path)}
              title={t("repoDetail.saveTitle")}
            >
              {t("common.save")}
            </Button>
          </div>
        )}
        <button
          type="button"
          onClick={onClose}
          className={cn(
            "rounded p-1 text-ink-dim hover:bg-hover hover:text-ink",
            isFile && !editor.loading && !editor.error ? "" : "ml-auto",
            "mr-1",
          )}
          title={t("common.close")}
        >
          <X className="size-3.5" />
        </button>
      </div>

      {editor.kind === "file" && editor.conflict && (
        <div className="flex shrink-0 items-center gap-2 border-b border-warn/30 bg-warn-soft px-3 py-1.5 text-[11px] text-warn">
          <AlertTriangle className="size-3.5 shrink-0" />
          <span className="min-w-0 flex-1">{t("repoDetail.conflictHint")}</span>
          <Button size="sm" variant="ghost" onClick={() => onReload(editor.path)}>
            {t("repoDetail.conflictReload")}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => onKeepDraft(editor.path)}>
            {t("repoDetail.conflictKeep")}
          </Button>
        </div>
      )}

      {/* 内容 */}
      {editor.error ? (
        <div className="min-h-0 flex-1 overflow-auto bg-sunken">
          <p className="m-4 rounded-md border border-neg/30 bg-neg-soft px-4 py-3 text-xs text-neg">
            {editor.error}
          </p>
        </div>
      ) : editor.loading ? (
        <div className="flex min-h-0 flex-1 items-center justify-center gap-2 bg-sunken text-sm text-ink-dim">
          <Loader2 className="size-4 animate-spin" /> {t("repoDetail.loading")}
        </div>
      ) : isDiff ? (
        <div className="min-h-0 flex-1 overflow-auto bg-sunken">
          {editor.diff ? (
            <DiffView diff={editor.diff} />
          ) : (
            <p className="px-4 py-16 text-center text-xs text-ink-faint">
              {t("repoDetail.noDiff")}
            </p>
          )}
        </div>
      ) : isFile && editor.draft !== null ? (
        <FileEditorView
          key={editor.path}
          path={editor.path}
          draft={editor.draft}
          truncated={editor.truncated}
          reveal={reveal}
          onDraftChange={onDraftChange}
          onSave={onSave}
          onRevealDone={onRevealDone}
        />
      ) : null}
    </div>
  );
}

function FileEditorView({
  path,
  draft,
  truncated,
  reveal,
  onDraftChange,
  onSave,
  onRevealDone,
}: {
  path: string;
  draft: string;
  truncated: boolean;
  reveal: Reveal | null;
  onDraftChange: (path: string, draft: string) => void;
  onSave: (path: string) => void;
  onRevealDone: () => void;
}) {
  const { t } = useTranslation();
  const taRef = useRef<HTMLTextAreaElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const preRef = useRef<HTMLPreElement>(null);
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [showReplace, setShowReplace] = useState(false);
  const [replaceValue, setReplaceValue] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [activeIdx, setActiveIdx] = useState(0);

  const heavy = draft.length > HEAVY_LIMIT;
  const highlighted = useMemo(
    () => (!heavy ? highlightCode(draft, path) : null),
    [draft, path, heavy],
  );
  const useOverlay = !!highlighted;

  const matches = useMemo(
    () => (findOpen && findQuery && !heavy ? findMatches(draft, findQuery, caseSensitive) : []),
    [draft, findQuery, caseSensitive, findOpen, heavy],
  );
  const matchesRef = useRef<MatchRange[]>([]);
  matchesRef.current = matches;
  const safeIdx = matches.length > 0 ? Math.min(activeIdx, matches.length - 1) : 0;
  const current = matches[safeIdx];

  useEffect(() => {
    setActiveIdx(0);
  }, [findQuery, caseSensitive]);

  function syncScroll(scrollTop: number, scrollLeft: number) {
    if (preRef.current) {
      preRef.current.scrollTop = scrollTop;
      preRef.current.scrollLeft = scrollLeft;
    }
    if (gutterRef.current) gutterRef.current.scrollTop = scrollTop;
  }

  function gotoLine(line0: number) {
    const ta = taRef.current;
    if (!ta) return;
    // 内容有 12px 顶部内边距，行位置要加上才是真实偏移。
    const top = line0 * LINE_H + EDITOR_PAD_TOP;
    if (top < ta.scrollTop || top > ta.scrollTop + ta.clientHeight - LINE_H * 2) {
      ta.scrollTop = Math.max(0, top - ta.clientHeight / 3);
      syncScroll(ta.scrollTop, ta.scrollLeft);
    }
  }

  function selectMatch(m: MatchRange | undefined) {
    const ta = taRef.current;
    if (!ta || !m) return;
    ta.focus();
    ta.setSelectionRange(m.start, m.end);
    gotoLine(offsetToLine(draft, m.start));
  }

  useEffect(() => {
    if (!findOpen) return;
    selectMatch(matchesRef.current[Math.min(activeIdx, Math.max(matchesRef.current.length - 1, 0))]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [findQuery, caseSensitive, activeIdx, findOpen]);

  // 从搜索结果跳转：选中并滚动到目标行。
  const revealedTs = useRef(0);
  useEffect(() => {
    if (!reveal || reveal.path !== path || reveal.ts === revealedTs.current) return;
    revealedTs.current = reveal.ts;
    const lines = draft.split("\n");
    const target = Math.max(0, Math.min(reveal.line - 1, lines.length - 1));
    const start = lines.slice(0, target).reduce((acc, l) => acc + l.length + 1, 0);
    const end = start + (lines[target]?.length ?? 0);
    const ta = taRef.current;
    if (ta) {
      ta.focus();
      ta.setSelectionRange(start, end);
      ta.scrollTop = Math.max(0, target * LINE_H + EDITOR_PAD_TOP - ta.clientHeight / 3);
      syncScroll(ta.scrollTop, ta.scrollLeft);
    }
    onRevealDone();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reveal, draft, path]);

  function step(delta: number) {
    if (matches.length === 0) return;
    const next = (activeIdx + delta + matches.length) % matches.length;
    setActiveIdx(next);
  }

  function replaceCurrent() {
    if (!current) return;
    onDraftChange(path, replaceAt(draft, current.start, current.end, replaceValue));
  }

  function replaceAll() {
    if (matches.length === 0) return;
    let next = "";
    let last = 0;
    for (const m of matches) {
      next += draft.slice(last, m.start) + replaceValue;
      last = m.end;
    }
    next += draft.slice(last);
    onDraftChange(path, next);
  }

  const lineCount = Math.max(draft.split("\n").length, 1);

  return (
    <div className="flex min-h-0 flex-1 flex-col bg-sunken">
      {truncated && (
        <p className="shrink-0 border-b border-warn/30 bg-warn-soft px-4 py-1.5 text-[11px] text-warn">
          {t("repoDetail.fileTooLarge")}
        </p>
      )}
      {findOpen && (
        <div className="ui-titlebar flex h-9 shrink-0 items-center gap-1.5 border-b border-line px-2.5">
          <input
            autoFocus
            value={findQuery}
            onChange={(event) => setFindQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                step(event.shiftKey ? -1 : 1);
              }
              if (event.key === "Escape") setFindOpen(false);
            }}
            placeholder={t("repoDetail.findPlaceholder")}
            className="ui-input h-6 w-44 rounded px-2 text-[11px] text-ink placeholder:text-ink-faint"
          />
          <span className="w-14 shrink-0 text-[10px] text-ink-dim">
            {findQuery
              ? matches.length
                ? `${safeIdx + 1}/${matches.length}`
                : t("repoDetail.noResults")
              : ""}
          </span>
          <button
            type="button"
            onClick={() => step(-1)}
            className="rounded p-1 text-ink-dim hover:bg-hover hover:text-ink"
            title={t("repoDetail.prevMatch")}
          >
            <ChevronUp className="size-3" />
          </button>
          <button
            type="button"
            onClick={() => step(1)}
            className="rounded p-1 text-ink-dim hover:bg-hover hover:text-ink"
            title={t("repoDetail.nextMatch")}
          >
            <ChevronDown className="size-3" />
          </button>
          <button
            type="button"
            onClick={() => setCaseSensitive((value) => !value)}
            className={cn(
              "h-6 w-6 shrink-0 rounded border text-[10px] font-semibold",
              caseSensitive
                ? "border-brand-line bg-brand-soft text-brand"
                : "border-transparent text-ink-dim hover:bg-hover",
            )}
            title={t("repoDetail.caseSensitive")}
          >
            Aa
          </button>
          <button
            type="button"
            onClick={() => setShowReplace((value) => !value)}
            className={cn(
              "rounded p-1",
              showReplace ? "text-brand" : "text-ink-dim hover:bg-hover hover:text-ink",
            )}
            title={t("repoDetail.replace")}
          >
            <Replace className="size-3.5" />
          </button>
          {showReplace && (
            <>
              <input
                value={replaceValue}
                onChange={(event) => setReplaceValue(event.target.value)}
                placeholder={t("repoDetail.replaceWith")}
                className="ui-input h-6 w-32 rounded px-2 text-[11px] text-ink placeholder:text-ink-faint"
              />
              <button
                type="button"
                onClick={replaceCurrent}
                disabled={!current}
                className="h-6 rounded border border-line px-1.5 text-[10px] text-ink hover:bg-hover disabled:opacity-40"
                title={t("repoDetail.replaceCurrent")}
              >
                {t("repoDetail.replace")}
              </button>
              <button
                type="button"
                onClick={replaceAll}
                disabled={matches.length === 0}
                className="h-6 rounded border border-line px-1.5 text-[10px] text-ink hover:bg-hover disabled:opacity-40"
                title={t("repoDetail.replaceAllTitle")}
              >
                {t("repoDetail.replaceAll")}
              </button>
            </>
          )}
          <button
            type="button"
            onClick={() => setFindOpen(false)}
            className="ml-auto rounded p-1 text-ink-dim hover:bg-hover hover:text-ink"
            title={t("repoDetail.closeEsc")}
          >
            <X className="size-3.5" />
          </button>
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <div
          ref={gutterRef}
          className="w-11 shrink-0 select-none overflow-hidden border-r border-line bg-sunken pt-3 text-right font-mono text-[10px] leading-[20px] text-ink-faint"
          aria-hidden
        >
          {Array.from({ length: lineCount }, (_, i) => (
            <div key={i} className="pr-2">
              {i + 1}
            </div>
          ))}
        </div>
        <div className="relative min-w-0 flex-1">
          {useOverlay && (
            <pre
              ref={preRef}
              aria-hidden
              className="hljs pointer-events-none absolute inset-0 overflow-hidden whitespace-pre px-3 pt-3 font-mono text-[11.5px] leading-[20px] [tab-size:4]"
            >
              <code dangerouslySetInnerHTML={{ __html: `${highlighted}\n` }} />
            </pre>
          )}
          <textarea
            ref={taRef}
            value={draft}
            readOnly={truncated}
            spellCheck={false}
            wrap="off"
            onChange={(event) => onDraftChange(path, event.target.value)}
            onScroll={(event) => syncScroll(event.currentTarget.scrollTop, event.currentTarget.scrollLeft)}
            onKeyDown={(event) => {
              if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
                event.preventDefault();
                onSave(path);
              }
              if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
                event.preventDefault();
                setFindOpen(true);
              }
              if (event.key === "F3") {
                event.preventDefault();
                step(event.shiftKey ? -1 : 1);
              }
            }}
            className={cn(
              "absolute inset-0 h-full w-full resize-none overflow-auto bg-transparent px-3 pt-3 font-mono text-[11.5px] leading-[20px] caret-ink outline-none [tab-size:4]",
              useOverlay
                ? "text-transparent selection:bg-brand-soft"
                : "text-ink selection:bg-brand-soft",
            )}
          />
        </div>
      </div>
    </div>
  );
}
