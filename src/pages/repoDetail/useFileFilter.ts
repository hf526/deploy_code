import { useEffect, useState } from "react";

import { api } from "../../lib/api";

/**
 * 文件树过滤：输入防抖查询文件列表。
 * `fsTick` 由外部文件变化监听驱动，变化时对当前关键词重新查询。
 */
export function useFileFilter(repoId: string, fsTick: number) {
  const [fileFilter, setFileFilter] = useState("");
  const [fileMatches, setFileMatches] = useState<string[] | null>(null);
  const [filterLoading, setFilterLoading] = useState(false);

  // 切换仓库时清空上一仓库的匹配结果，避免加载期间短暂显示旧仓库文件。
  useEffect(() => {
    setFileMatches(null);
  }, [repoId]);

  useEffect(() => {
    const query = fileFilter.trim();
    if (!query) {
      setFileMatches(null);
      setFilterLoading(false);
      return;
    }
    let cancelled = false;
    setFilterLoading(true);
    const timer = window.setTimeout(() => {
      void api
        .findFiles(repoId, query)
        .then((files) => {
          if (!cancelled) setFileMatches(files);
        })
        .catch(() => {
          if (!cancelled) setFileMatches([]);
        })
        .finally(() => {
          if (!cancelled) setFilterLoading(false);
        });
    }, 220);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [fileFilter, repoId, fsTick]);

  return { fileFilter, setFileFilter, fileMatches, filterLoading };
}
