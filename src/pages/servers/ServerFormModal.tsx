import { useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderOpen, Wifi } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";

import { Button, Field, Input, Modal, Select } from "../../components/ui";
import { api } from "../../lib/api";
import { useApp } from "../../lib/store";
import type { ServerConfig } from "../../lib/types";
import { newServerTemplate } from "../../lib/utils";

/** 服务器表单：从 ServersPage 搬过来，字段与校验口径没变。 */
export function ServerFormModal({
  open: isOpen,
  initial,
  onClose,
  onSaved,
}: {
  open: boolean;
  initial: ServerConfig | null;
  onClose: () => void;
  onSaved: () => void | Promise<void>;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const [draft, setDraft] = useState<ServerConfig>(newServerTemplate);
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [initialized, setInitialized] = useState(false);

  if (isOpen && !initialized) {
    setDraft(initial ? { ...initial, auth: { ...initial.auth } } : newServerTemplate());
    setInitialized(true);
  }
  if (!isOpen && initialized) {
    setInitialized(false);
  }

  async function browseKey() {
    const selected = await open({ multiple: false, title: t("servers.keyBrowseTitle") });
    if (typeof selected === "string") {
      setDraft((current) =>
        current.auth.type === "privateKey"
          ? { ...current, auth: { ...current.auth, keyPath: selected } }
          : current,
      );
    }
  }

  async function save() {
    if (busy || testing) return;
    if (!draft.name.trim()) return toast("error", t("servers.errorName"));
    if (!draft.host.trim()) return toast("error", t("servers.errorHost"));
    if (!draft.username.trim()) return toast("error", t("servers.errorUsername"));
    if (draft.auth.type === "password" && !draft.auth.password) {
      return toast("error", t("servers.errorPassword"));
    }
    if (draft.auth.type === "privateKey" && !draft.auth.keyPath.trim()) {
      return toast("error", t("servers.errorKey"));
    }

    setBusy(true);
    try {
      const saved = await api.saveServer(draft);
      toast("success", initial ? t("servers.updated") : t("servers.added"));
      setDraft(saved);
      await onSaved();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setBusy(false);
    }
  }

  async function test() {
    if (busy || testing) return;
    setTesting(true);
    try {
      const message = await api.testServer(draft);
      toast("success", message);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setTesting(false);
    }
  }

  /** 保存 / 测试进行中禁止关闭，避免异步结果落到另一个服务器的表单上。 */
  function requestClose() {
    if (busy || testing) return;
    onClose();
  }

  return (
    <Modal
      open={isOpen}
      onClose={requestClose}
      title={initial ? t("servers.editTitle", { name: initial.name }) : t("servers.addTitle")}
      subtitle={t("servers.formSubtitle")}
      onEnter={() => {
        if (!busy && !testing) void save();
      }}
      footer={
        <>
          <Button
            variant="secondary"
            loading={testing}
            disabled={busy}
            icon={<Wifi className="size-4" />}
            onClick={() => void test()}
          >
            {t("servers.testConnection")}
          </Button>
          <Button variant="secondary" disabled={busy || testing} onClick={requestClose}>
            {t("common.cancel")}
          </Button>
          <Button loading={busy} disabled={testing} onClick={() => void save()}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="grid grid-cols-2 gap-4">
          <Field label={t("servers.name")} required>
            <Input
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder={t("servers.namePlaceholder")}
            />
          </Field>
          <Field label={t("servers.host")} required>
            <Input
              value={draft.host}
              onChange={(event) => setDraft({ ...draft, host: event.target.value })}
              placeholder={t("servers.hostPlaceholder")}
            />
          </Field>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <Field label={t("servers.port")}>
            <Input
              type="number"
              value={draft.port}
              onChange={(event) => setDraft({ ...draft, port: Number(event.target.value) || 22 })}
            />
          </Field>
          <Field label={t("servers.username")} required>
            <Input
              value={draft.username}
              onChange={(event) => setDraft({ ...draft, username: event.target.value })}
              placeholder="root"
            />
          </Field>
        </div>

        <Field label={t("servers.authType")}>
          <Select
            value={draft.auth.type}
            onChange={(event) => {
              const type = event.target.value as "password" | "privateKey";
              if (type === draft.auth.type) return;
              setDraft({
                ...draft,
                auth:
                  type === "password"
                    ? { type: "password", password: "" }
                    : { type: "privateKey", keyPath: "", passphrase: null },
              });
            }}
          >
            <option value="password">{t("servers.passwordAuth")}</option>
            <option value="privateKey">{t("servers.keyAuth")}</option>
          </Select>
        </Field>

        {draft.auth.type === "password" ? (
          <Field label={t("servers.password")} required>
            <Input
              type="password"
              value={draft.auth.password}
              onChange={(event) =>
                setDraft({ ...draft, auth: { type: "password", password: event.target.value } })
              }
              placeholder={t("servers.passwordPlaceholder")}
            />
          </Field>
        ) : (
          <>
            <Field label={t("servers.keyPath")} required>
              <div className="flex gap-2">
                <Input
                  value={draft.auth.keyPath}
                  onChange={(event) =>
                    setDraft({
                      ...draft,
                      auth: {
                        type: "privateKey",
                        keyPath: event.target.value,
                        passphrase: draft.auth.type === "privateKey" ? draft.auth.passphrase : null,
                      },
                    })
                  }
                  placeholder="C:\Users\you\.ssh\id_rsa"
                />
                <Button
                  variant="secondary"
                  icon={<FolderOpen className="size-4" />}
                  onClick={() => void browseKey()}
                >
                  {t("servers.browse")}
                </Button>
              </div>
            </Field>
            <Field label={t("servers.keyPassphrase")} hint={t("servers.keyPassphraseHint")}>
              <Input
                type="password"
                value={draft.auth.type === "privateKey" ? (draft.auth.passphrase ?? "") : ""}
                onChange={(event) =>
                  setDraft({
                    ...draft,
                    auth: {
                      type: "privateKey",
                      keyPath: draft.auth.type === "privateKey" ? draft.auth.keyPath : "",
                      passphrase: event.target.value || null,
                    },
                  })
                }
              />
            </Field>
          </>
        )}

        <Field label={t("servers.defaultTargetDir")} hint={t("servers.defaultTargetDirHint")}>
          <Input
            value={draft.defaultTargetDir}
            onChange={(event) => setDraft({ ...draft, defaultTargetDir: event.target.value })}
            placeholder="/opt/apps/my-app"
          />
        </Field>
      </div>
    </Modal>
  );
}
