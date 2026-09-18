import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button, Field, Input, Modal } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";

export interface NginxEditorState {
  name: string;
  content: string;
  isNew: boolean;
  /** 打开弹窗时的目标快照：即使期间切换了服务器/容器，保存仍落在原目标上。 */
  serverId: string;
  container: string;
  dir: string;
}

interface Props {
  state: NginxEditorState;
  onClose: () => void;
  onSaved: (name: string) => void;
}

export function NginxConfigModal({ state, onClose, onSaved }: Props) {
  const { t } = useTranslation();
  const toast = useApp((store) => store.toast);
  const [name, setName] = useState(state.name);
  const [content, setContent] = useState(state.content);
  const [saving, setSaving] = useState(false);

  async function handleSave() {
    if (saving) return;
    const trimmed = name.trim();
    if (!trimmed) {
      toast("error", t("nginx.errorName"));
      return;
    }
    setSaving(true);
    try {
      const message = await api.saveNginxConfig({
        serverId: state.serverId,
        container: state.container,
        dir: state.dir,
        name: trimmed,
        content,
      });
      toast("success", message);
      onSaved(trimmed);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={state.isNew ? t("nginx.newTitle") : t("nginx.editTitle", { name: state.name })}
      subtitle={t("nginx.modalSubtitle", { container: state.container })}
      width="max-w-3xl"
      footer={
        <div className="flex justify-end gap-2">
          <Button variant="secondary" disabled={saving} onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button loading={saving} onClick={() => void handleSave()}>
            {t("common.save")}
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-4">
        <Field
          label={t("nginx.fileName")}
          hint={state.isNew ? undefined : t("nginx.fileNameLocked")}
          required
        >
          <Input
            value={name}
            disabled={!state.isNew || saving}
            placeholder={t("nginx.fileNamePlaceholder")}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <Field label={t("nginx.content")} hint={t("nginx.editorHint")}>
          <textarea
            value={content}
            disabled={saving}
            spellCheck={false}
            className="ui-input min-h-[420px] w-full resize-y rounded-md px-3 py-2 font-mono text-xs leading-relaxed text-ink"
            onChange={(event) => setContent(event.target.value)}
          />
        </Field>
      </div>
    </Modal>
  );
}
