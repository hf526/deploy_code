import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button, Field, Input, Modal } from "./ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { RepoInfo } from "../lib/types";

/** 绑定 / 修改仓库的 Git 远端地址（origin）。 */
export function BindRemoteModal({
  repo,
  onClose,
  onSaved,
}: {
  repo: RepoInfo | null;
  onClose: () => void;
  onSaved: (repo: RepoInfo) => void;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const [url, setUrl] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setUrl(repo?.remote ?? "");
    setSaving(false);
  }, [repo?.id, repo?.remote]);

  async function handleSave() {
    if (!repo) return;
    const value = url.trim();
    if (!value) {
      toast("error", t("remoteModal.urlRequired"));
      return;
    }
    setSaving(true);
    try {
      const saved = await api.setRepoRemote(repo.id, value);
      onSaved(saved);
      toast("success", repo.remote ? t("remoteModal.updated") : t("remoteModal.bound"));
      onClose();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      open={!!repo}
      onClose={onClose}
      title={repo?.remote ? t("remoteModal.editTitle") : t("remoteModal.bindTitle")}
      subtitle={repo?.name}
      width="max-w-lg"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button loading={saving} onClick={() => void handleSave()}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <Field label={t("remoteModal.url")} hint={t("remoteModal.urlHint")} required>
        <Input
          autoFocus
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="git@github.com:user/repo.git"
          onKeyDown={(event) => {
            if (event.key === "Enter") void handleSave();
          }}
        />
      </Field>
      <p className="text-[11px] leading-relaxed text-ink-faint">
        {repo && !repo.isRepo
          ? t("remoteModal.initDescription")
          : t("remoteModal.description", { action: repo?.remote ? "set-url" : "add" })}
      </p>
    </Modal>
  );
}
