import { useState } from "react";
import { useTranslation } from "react-i18next";
import { RotateCcw } from "lucide-react";

import { Button, Checkbox, Field, Input, Modal, Select } from "../../components/ui";
import { useApp } from "../../lib/store";
import { humanSize } from "../../lib/utils";

/**
 * 把本机已有的容器备份包再恢复到一台服务器。
 * 目标目录留空时沿用备份包内记录的原目录；同名数据卷会被包里的内容覆盖。
 */
export function RestoreBundleModal({
  bundlePath,
  bundleSize,
  project,
  busy,
  onClose,
  onSubmitted,
}: {
  bundlePath: string;
  bundleSize: number;
  project: string;
  busy: boolean;
  onClose: () => void;
  onSubmitted: () => void;
}) {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const restore = useApp((state) => state.restoreContainerBundle);
  const toast = useApp((state) => state.toast);

  const [serverId, setServerId] = useState("");
  const [targetDir, setTargetDir] = useState("");
  const [startServices, setStartServices] = useState(true);
  const [acknowledged, setAcknowledged] = useState(false);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit() {
    if (!serverId) {
      toast("error", t("containers.pickTarget"));
      return;
    }
    const dir = targetDir.trim();
    if (dir && !dir.startsWith("/")) {
      toast("error", t("containers.targetDirRequired"));
      return;
    }
    setSubmitting(true);
    try {
      await restore({
        bundlePath,
        target: { serverId, targetDir: dir, startServices },
      });
      onSubmitted();
    } catch {
      // store 已弹出错误提示。
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal
      open
      onClose={submitting ? () => undefined : onClose}
      title={t("containers.restoreTitle")}
      subtitle={`${project} · ${humanSize(bundleSize)}`}
      width="max-w-lg"
      footer={
        <>
          <Button variant="secondary" disabled={submitting} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button
            variant="danger"
            icon={<RotateCcw className="size-4" />}
            loading={submitting || busy}
            disabled={!acknowledged || !serverId || busy}
            onClick={() => void handleSubmit()}
          >
            {t("containers.restoreConfirm")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="break-all font-mono text-[11px] text-ink-faint" title={bundlePath}>
          {bundlePath}
        </p>

        <Field label={t("containers.targetServer")} required>
          <Select
            value={serverId}
            disabled={submitting || servers.length === 0}
            onChange={(event) => setServerId(event.target.value)}
          >
            {servers.length === 0 ? (
              <option value="">{t("containers.noServers")}</option>
            ) : (
              <>
                <option value="">{t("containers.pickTarget")}</option>
                {servers.map((server) => (
                  <option key={server.id} value={server.id}>
                    {server.name} · {server.host}
                  </option>
                ))}
              </>
            )}
          </Select>
        </Field>

        <Field label={t("containers.targetDir")} hint={t("containers.restoreDirHint")}>
          <Input
            value={targetDir}
            disabled={submitting}
            placeholder={t("containers.restoreDirPlaceholder")}
            onChange={(event) => setTargetDir(event.target.value)}
          />
        </Field>

        <Checkbox checked={startServices} disabled={submitting} onChange={setStartServices}>
          {t("containers.startServices")}
        </Checkbox>

        <Checkbox
          checked={acknowledged}
          disabled={submitting}
          onChange={setAcknowledged}
          className="items-start"
        >
          <span className="leading-relaxed">{t("containers.restoreAcknowledge", { project })}</span>
        </Checkbox>
      </div>
    </Modal>
  );
}
