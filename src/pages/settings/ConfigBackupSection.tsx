import { useState } from "react";
import { FileJson, Upload } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Card, Modal, SectionTitle } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { ImportPreview } from "../../lib/types";

/** 配置导入导出：导出必然已脱敏，所以导入必须先预览、用户确认后才落盘。 */
export function ConfigBackupSection() {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const loadAll = useApp((state) => state.loadAll);

  const [importDraft, setImportDraft] = useState<{ text: string; preview: ImportPreview } | null>(
    null,
  );
  const [importBusy, setImportBusy] = useState(false);

  async function handleExportConfig() {
    try {
      const json = await api.exportConfig();
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `deploycode-config-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
      toast("success", t("settings.configBackup.exported"));
    } catch (error) {
      toast("error", String(error));
    }
  }

  /** 选择文件后只做预览，不落盘。 */
  async function handleImportConfig(file: File) {
    try {
      const text = await file.text();
      const preview = await api.previewConfigImport(text);
      setImportDraft({ text, preview });
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleApplyImport() {
    if (!importDraft) return;
    setImportBusy(true);
    try {
      await api.importConfig(importDraft.text);
      setImportDraft(null);
      // 导入会改动服务器 / 仓库 / 各类配置，整份重载才能保证界面与落盘一致。
      await loadAll();
      toast("success", t("settings.configBackup.imported"));
    } catch (error) {
      toast("error", String(error));
    } finally {
      setImportBusy(false);
    }
  }

  return (
    <section>
      <SectionTitle
        title={t("settings.configBackup.title")}
        description={t("settings.configBackup.description")}
      />
      <Card className="flex items-center gap-3 p-5">
        <div className="flex items-center gap-2">
          <Button
            variant="secondary"
            icon={<FileJson className="size-4" />}
            onClick={handleExportConfig}
          >
            {t("settings.configBackup.export")}
          </Button>
          <label className="cursor-pointer">
            <input
              type="file"
              accept=".json"
              className="hidden"
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file) {
                  void handleImportConfig(file);
                  e.target.value = "";
                }
              }}
            />
            <Button variant="secondary" icon={<Upload className="size-4" />}>
              {t("settings.configBackup.import")}
            </Button>
          </label>
        </div>
      </Card>

      {importDraft && (
        <Modal
          open
          onClose={() => setImportDraft(null)}
          title={t("settings.configBackup.confirmImport")}
          subtitle={t("settings.configBackup.preview.exportedAt", {
            time: importDraft.preview.exportedAt,
          })}
          footer={
            <>
              <Button
                variant="secondary"
                disabled={importBusy}
                onClick={() => setImportDraft(null)}
              >
                {t("common.cancel")}
              </Button>
              <Button
                icon={<Upload className="size-4" />}
                loading={importBusy}
                onClick={() => void handleApplyImport()}
              >
                {t("settings.configBackup.import")}
              </Button>
            </>
          }
        >
          <div className="flex flex-col gap-3">
            {importRows(importDraft.preview).length === 0 ? (
              <p className="text-xs text-ink-dim">{t("settings.configBackup.preview.nothing")}</p>
            ) : (
              <table className="w-full text-left text-xs">
                <thead>
                  <tr className="border-b border-line text-[11px] uppercase tracking-wide text-ink-faint">
                    <th className="py-2 font-medium">{t("settings.configBackup.preview.kind")}</th>
                    <th className="py-2 text-right font-medium">
                      {t("settings.configBackup.preview.added")}
                    </th>
                    <th className="py-2 text-right font-medium">
                      {t("settings.configBackup.preview.overwritten")}
                    </th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-line">
                  {importRows(importDraft.preview).map((row) => (
                    <tr key={row.key}>
                      <td className="py-2 text-ink">{t(`settings.configBackup.kinds.${row.key}`)}</td>
                      <td className="py-2 text-right tabular-nums text-ink-dim">{row.counts.added}</td>
                      <td className="py-2 text-right tabular-nums text-ink-dim">
                        {row.counts.overwritten}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            {importDraft.preview.keptLocalSecrets > 0 && (
              <p className="rounded-md border border-warn/30 bg-warn-soft px-3 py-2 text-[11px] leading-relaxed text-warn">
                {t("settings.configBackup.preview.keptSecrets", {
                  count: importDraft.preview.keptLocalSecrets,
                })}
              </p>
            )}
            <p className="text-[11px] leading-relaxed text-ink-faint">
              {t("settings.configBackup.preview.note")}
            </p>
          </div>
        </Modal>
      )}
    </section>
  );
}

/** 预览表格里只显示真正有变动的配置类别，全零的行不留占位。 */
function importRows(preview: ImportPreview) {
  return (
    [
      { key: "servers", counts: preview.servers },
      { key: "repos", counts: preview.repos },
      { key: "backupTargets", counts: preview.backupTargets },
      { key: "deployConfigs", counts: preview.deployConfigs },
      { key: "backupConfigs", counts: preview.backupConfigs },
      { key: "pagesConfigs", counts: preview.pagesConfigs },
      { key: "containerConfigs", counts: preview.containerConfigs },
    ] as const
  ).filter((row) => row.counts.added + row.counts.overwritten > 0);
}
