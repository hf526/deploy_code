import { useEffect, useState } from "react";
import { FolderOpen, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Card, SectionTitle } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import { humanSize } from "../../lib/utils";
import type { OrphanBundle } from "../../lib/types";

/**
 * 数据目录与「未认领的备份包」。
 * 孤儿包跨数据库备份与容器备份两类，任何一边的记录都指不到它们，只能在这里手动收。
 */
export function StorageSection() {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);

  const [dataDir, setDataDir] = useState("");
  const [orphans, setOrphans] = useState<OrphanBundle[]>([]);
  const [orphanBusy, setOrphanBusy] = useState(false);

  async function loadOrphans() {
    try {
      setOrphans(await api.listOrphanBundles());
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleCleanOrphans() {
    if (!orphans.length) return;
    setOrphanBusy(true);
    try {
      const result = await api.deleteOrphanBundles(orphans.map((item) => item.path));
      toast(
        "success",
        t("settings.orphanCleaned", {
          count: result.deleted,
          size: humanSize(result.freedBytes),
        }),
      );
      await loadOrphans();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setOrphanBusy(false);
    }
  }

  useEffect(() => {
    // 本机目录扫描，进页面读一次就够，不轮询。
    void loadOrphans();
    void api
      .getDataDir()
      .then(setDataDir)
      .catch(() => undefined);
  }, []);

  return (
    <section>
      <SectionTitle
        title={t("settings.storage.title")}
        description={t("settings.storage.description")}
      />
      <Card className="flex items-center gap-3 p-5">
        <p className="min-w-0 flex-1 truncate font-mono text-xs text-ink-dim" title={dataDir}>
          {dataDir || t("common.loading")}
        </p>
        <Button
          variant="secondary"
          icon={<FolderOpen className="size-4" />}
          disabled={!dataDir}
          onClick={() => void api.revealPath(dataDir).catch((e) => toast("error", String(e)))}
        >
          {t("settings.storage.openDir")}
        </Button>
      </Card>

      <Card className="mt-4 p-4">
        <SectionTitle title={t("settings.orphanBundles")} />
        <p className="mt-2 text-[11px] leading-relaxed text-ink-dim">
          {t("settings.orphanBundlesHint")}
        </p>
        {orphans.length === 0 ? (
          <p className="mt-2 text-[11px] text-pos">{t("settings.orphanBundlesNone")}</p>
        ) : (
          <>
            <div className="mt-3 space-y-1.5">
              {orphans.slice(0, 50).map((item) => (
                <div
                  key={item.path}
                  className="flex items-center justify-between gap-3 text-[11px]"
                >
                  <span className="min-w-0 truncate text-ink" title={item.path}>
                    {item.kind === "container"
                      ? t("settings.orphanKindContainer")
                      : t("settings.orphanKindDatabase")}{" "}
                    · {item.fileName}
                  </span>
                  <span className="shrink-0 text-ink-dim">{humanSize(item.sizeBytes)}</span>
                </div>
              ))}
              {orphans.length > 50 && (
                <p className="text-[11px] text-ink-dim">
                  {t("settings.orphanMore", { count: orphans.length - 50 })}
                </p>
              )}
            </div>
            <div className="mt-3 flex items-center justify-between gap-3 border-t border-line pt-3">
              <p className="text-[11px] text-warn">
                {t("settings.orphanTotal", {
                  count: orphans.length,
                  size: humanSize(orphans.reduce((sum, item) => sum + item.sizeBytes, 0)),
                })}
              </p>
              <Button
                variant="secondary"
                size="sm"
                loading={orphanBusy}
                icon={<Trash2 className="size-3.5" />}
                onClick={() => void handleCleanOrphans()}
              >
                {t("settings.orphanClean")}
              </Button>
            </div>
          </>
        )}
      </Card>
    </section>
  );
}
