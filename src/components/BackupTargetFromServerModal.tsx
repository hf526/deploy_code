import { useEffect, useState } from "react";
import { Plus } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button, Field, Input, Modal, Select } from "./ui";
import type { BackupTarget, ServerConfig } from "../lib/types";
import { maskUrlPassword } from "../lib/utils";

const DEFAULT_PG_PORT = 5432;

function hostForUrl(host: string): string {
  const value = host.trim();
  if (value.includes(":") && !value.startsWith("[")) return `[${value}]`;
  return value;
}

export function databaseUrlFromServer(
  server: ServerConfig,
  database: string,
  username: string,
  password: string,
  port: number,
): string {
  const user = encodeURIComponent(username.trim());
  const pass = password ? `:${encodeURIComponent(password)}` : "";
  return `postgresql://${user}${pass}@${hostForUrl(server.host)}:${port}/${encodeURIComponent(database.trim())}`;
}

export function BackupTargetFromServerModal({
  open,
  servers,
  onClose,
  onAdd,
}: {
  open: boolean;
  servers: ServerConfig[];
  onClose: () => void;
  onAdd: (target: BackupTarget) => void;
}) {
  const { t } = useTranslation();
  const [serverId, setServerId] = useState("");
  const [name, setName] = useState("");
  const [database, setDatabase] = useState("");
  const [username, setUsername] = useState("postgres");
  const [password, setPassword] = useState("");
  const [port, setPort] = useState(String(DEFAULT_PG_PORT));

  const selected = servers.find((server) => server.id === serverId) ?? null;

  function applyServer(server: ServerConfig) {
    setServerId(server.id);
    setName(server.name);
    setDatabase(server.dbBackup?.database ?? "");
    setUsername(server.dbBackup?.username || "postgres");
    setPassword(server.dbBackup?.password ?? "");
  }

  useEffect(() => {
    if (!open) return;
    if (servers.length > 0) applyServer(servers[0]);
    else {
      setServerId("");
      setName("");
      setDatabase("");
      setUsername("postgres");
      setPassword("");
    }
    setPort(String(DEFAULT_PG_PORT));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const portNumber = Number(port);
  // 非法输入（空 / NaN / 超范围）直接判定无效，不能静默回退成 5432。
  const portValid = Number.isInteger(portNumber) && portNumber > 0 && portNumber < 65536;
  const canAdd = !!selected && database.trim() !== "" && username.trim() !== "" && portValid;
  const preview =
    selected && canAdd
      ? databaseUrlFromServer(selected, database, username, password, portNumber)
      : "";

  function handleAdd() {
    if (!selected || !canAdd) return;
    onAdd({
      id: "",
      name: name.trim() || selected.name,
      url: databaseUrlFromServer(selected, database, username, password, portNumber),
    });
    onClose();
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("settings.backupTargets.fromServerTitle")}
      subtitle={t("settings.backupTargets.fromServerSubtitle")}
      width="max-w-lg"
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button disabled={!canAdd} icon={<Plus className="size-4" />} onClick={handleAdd}>
            {t("settings.backupTargets.fromServerAdd")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("backup.server")} required>
          <Select
            value={serverId}
            onChange={(event) => {
              const server = servers.find((item) => item.id === event.target.value);
              if (server) applyServer(server);
            }}
          >
            {servers.map((server) => (
              <option key={server.id} value={server.id}>
                {server.name} ({server.username}@{server.host})
              </option>
            ))}
          </Select>
        </Field>

        <div className="grid grid-cols-2 gap-4">
          <Field label={t("settings.backupTargets.fromServerDb")} required>
            <Input
              value={database}
              onChange={(event) => setDatabase(event.target.value)}
              placeholder="app"
            />
          </Field>
          <Field label={t("settings.backupTargets.fromServerPort")}>
            <Input
              type="number"
              value={port}
              onChange={(event) => setPort(event.target.value)}
            />
          </Field>
        </div>

        <div className="grid grid-cols-2 gap-4">
          <Field label={t("settings.backupTargets.fromServerUser")} required>
            <Input
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              placeholder="postgres"
            />
          </Field>
          <Field label={t("settings.backupTargets.fromServerPassword")}>
            <Input
              type="password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </Field>
        </div>

        <Field
          label={t("settings.backupTargets.fromServerTargetName")}
          hint={t("settings.backupTargets.fromServerTargetNameHint")}
        >
          <Input value={name} onChange={(event) => setName(event.target.value)} />
        </Field>

        {preview && (
          <div className="rounded-md border border-line bg-sunken px-3 py-2">
            <p className="text-[10px] uppercase tracking-wide text-ink-faint">
              {t("settings.backupTargets.fromServerPreview")}
            </p>
            <p className="mt-1 break-all font-mono text-[11px] text-ink-dim">
              {maskUrlPassword(preview)}
            </p>
          </div>
        )}

        <p className="text-[11px] leading-relaxed text-ink-faint">
          {t("settings.backupTargets.fromServerHint")}
        </p>
      </div>
    </Modal>
  );
}
