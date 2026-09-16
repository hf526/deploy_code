import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { api } from "../../lib/api";
import type { Branch } from "../../lib/types";

/**
 * 仓库分支列表：切换仓库时重新加载，并用 cancelled 标记丢弃过期响应。
 * `onError` 由调用方按需提示（部署配置弹窗需要，Pages 弹窗静默处理）。
 */
export function useRepoBranches(repoId: string, onError?: (error: unknown) => void) {
  const { t } = useTranslation();
  const [branches, setBranches] = useState<Branch[]>([]);
  const [loaded, setLoaded] = useState(false);

  const onErrorRef = useRef(onError);
  useEffect(() => {
    onErrorRef.current = onError;
  });

  useEffect(() => {
    if (!repoId) {
      setBranches([]);
      setLoaded(false);
      return;
    }
    let cancelled = false;
    // 切换仓库先清空，避免旧仓库分支在选择器里短暂残留。
    setBranches([]);
    setLoaded(false);
    void api
      .listBranches(repoId, false)
      .then((list) => {
        if (!cancelled) {
          setBranches(list);
          setLoaded(true);
        }
      })
      .catch((error) => {
        if (!cancelled) onErrorRef.current?.(error);
      });
    return () => {
      cancelled = true;
    };
  }, [repoId]);

  const branchChoices = useMemo(
    () =>
      branches.map((branch) => ({
        value: branch.name,
        label: branch.name,
        hint: branch.isCurrent ? t("deploy.current") : undefined,
      })),
    [branches, t],
  );

  return { branches, branchChoices, loaded };
}
