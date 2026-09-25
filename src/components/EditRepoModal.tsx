import { useEffect, useState } from "react";
import { FolderInput } from "lucide-react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";

import { Button, Field, Input, Modal } from "./ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { RepoInfo } from "../lib/types";

/** 编辑仓库的显示名与本地目录：目录被移动/改名后用它重新指向，避免只能删掉重加。 */
export function EditRepoModal({
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
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setName(repo?.name ?? "");
    setPath(repo?.path ?? "");
    setSaving(false);
  }, [repo?.id, repo?.name, repo?.path]);

  async function pickDir() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t("repos.pickDirTitle"),
        defaultPath: path || undefined,
      });
      if (typeof selected === "string") setPath(selected);
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleSave() {
    if (!repo) return;
    const nextName = name.trim();
    const nextPath = path.trim();
    if (!nextName) {
      toast("error", t("repos.nameRequired"));
      return;
    }
    if (!nextPath) {
      toast("error", t("repos.pathRequired"));
      return;
    }
    setSaving(true);
    try {
      // 路径没变时不提交：后端会拒绝仍然失效的旧目录，改名会被误挡。
      const saved = await api.updateRepo({
        repoId: repo.id,
        name: nextName,
        path: nextPath === repo.path ? null : nextPath,
      });
      onSaved(saved);
      toast("success", t("repos.saved"));
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
      title={t("repos.editTitle")}
      subtitle={repo && !repo.pathExists ? t("repos.pathUnavailable") : undefined}
      width="max-w-lg"
      footer={
        <>
          <Button variant="secondary" disabled={saving} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button loading={saving} onClick={() => void handleSave()}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("repos.nameField")} required>
          <Input
            autoFocus
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t("repos.namePlaceholder")}
            onKeyDown={(event) => {
              if (event.key === "Enter") void handleSave();
            }}
          />
        </Field>
        <Field label={t("repos.dirField")} required hint={t("repos.dirHint")}>
          <div className="flex items-center gap-2">
            <Input
              value={path}
              onChange={(event) => setPath(event.target.value)}
              placeholder={t("repos.dirPlaceholder")}
              className="font-mono"
              onKeyDown={(event) => {
                if (event.key === "Enter") void handleSave();
              }}
            />
            <Button
              variant="secondary"
              icon={<FolderInput className="size-4" />}
              onClick={() => void pickDir()}
            >
              {t("repos.select")}
            </Button>
          </div>
        </Field>
        <p className="text-[11px] leading-relaxed text-ink-faint">
          {repo && !repo.pathExists ? t("repos.dirDeadNote") : t("repos.dirNote")}
        </p>
      </div>
    </Modal>
  );
}
