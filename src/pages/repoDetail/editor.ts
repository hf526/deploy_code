/** 编辑器行高（px），与 leading-[20px] 保持一致。 */
export const LINE_H = 20;
/** 编辑器内容顶部内边距（px），与 pt-3 保持一致。 */
export const EDITOR_PAD_TOP = 12;
/** 超过该字符数不做高亮/查找，避免卡顿。 */
export const HEAVY_LIMIT = 60_000;

export interface MatchRange {
  start: number;
  end: number;
}

export function findMatches(text: string, query: string, caseSensitive: boolean): MatchRange[] {
  if (!query) return [];
  const ranges: MatchRange[] = [];
  if (caseSensitive) {
    let i = text.indexOf(query);
    while (i !== -1) {
      ranges.push({ start: i, end: i + query.length });
      i = text.indexOf(query, i + query.length);
    }
    return ranges;
  }
  const lowerText = text.toLowerCase();
  const lowerQuery = query.toLowerCase();
  let i = lowerText.indexOf(lowerQuery);
  while (i !== -1) {
    const end = i + query.length;
    // 大小写折叠可能改变长度，校验切片确实匹配才接受该位置。
    if (text.slice(i, end).toLowerCase() === lowerQuery) {
      ranges.push({ start: i, end });
    }
    i = lowerText.indexOf(lowerQuery, i + Math.max(query.length, 1));
  }
  return ranges;
}

export function offsetToLine(text: string, offset: number): number {
  let line = 0;
  for (let i = 0; i < offset && i < text.length; i++) {
    if (text.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

export function replaceAt(text: string, start: number, end: number, value: string): string {
  return text.slice(0, start) + value + text.slice(end);
}

/** 编辑区当前显示的内容：空 / 文件编辑 / diff 预览。 */
export type EditorView =
  | { kind: "none" }
  | {
      kind: "file";
      path: string;
      content: string | null;
      draft: string | null;
      truncated: boolean;
      error: string | null;
      loading: boolean;
      saving: boolean;
      /** 磁盘版本已被外部修改，且本地有未保存草稿：保存会覆盖磁盘改动。 */
      conflict: boolean;
    }
  | { kind: "diff"; path: string; diff: string | null; error: string | null; loading: boolean };

/** 从搜索结果跳转到文件时的一次性定位请求（ts 变化时触发重新定位）。 */
export type Reveal = { path: string; line: number; ts: number };
