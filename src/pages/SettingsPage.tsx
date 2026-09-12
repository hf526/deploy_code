import { useEffect, useRef, useState } from "react";
import { FolderOpen, Plus, Save, Settings2, Trash2 } from "lucide-react";

import { Button, Card, Field, Input, Page, SectionTitle, Select } from "../components/ui";
import { ThemeToggle } from "../components/ThemeToggle";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { BackupTarget, Settings } from "../lib/types";

export default function SettingsPage() {
  const settings = useApp((state) => state.settings);
  const setSettings = useApp((state) => state.setSettings);
  const backupTargets = useApp((state) => state.backupTargets);
  const refreshBackupTargets = useApp((state) => state.refreshBackupTargets);
  const refreshServers = useApp((state) => state.refreshServers);
  const toast = useApp((state) => state.toast);

  const [dataDir, setDataDir] = useState("");
  const [draft, setDraft] = useState<Settings>(settings);
  const [targetDraft, setTargetDraft] = useState<BackupTarget[]>(backupTargets);

  useEffect(() => {
    void api
      .getDataDir()
      .then(setDataDir)
      .catch(() => undefined);
  }, []);

  const previousSettings = useRef(settings);

  // 只把真正发生变化的字段同步进草稿，避免保存目标等操作覆盖页面上其他未保存的编辑。
  useEffect(() => {
    const previous = previousSettings.current;
    previousSettings.current = settings;
    setDraft((current) => {
      const next: Settings = { ...current };
      const patch = next as Record<keyof Settings, Settings[keyof Settings]>;
      for (const key of Object.keys(settings) as (keyof Settings)[]) {
        if (settings[key] !== previous[key]) patch[key] = settings[key];
      }
      return next;
    });
  }, [settings]);

  useEffect(() => {
    setTargetDraft(backupTargets);
  }, [backupTargets]);

  async function handleSaveSettings() {
    try {
      const saved = await api.saveSettings({
        ...draft,
        scriptDir: draft.scriptDir.trim() || "docker",
        connectTimeoutSecs: Math.max(3, Number(draft.connectTimeoutSecs) || 15),
        scriptTimeoutSecs: Math.max(10, Number(draft.scriptTimeoutSecs) || 1800),
        historyLimit: Math.max(20, Number(draft.historyLimit) || 500),
        supabaseUrl: draft.supabaseUrl.trim(),
        defaultBackupTargetId: draft.defaultBackupTargetId || null,
        backupHistoryLimit: Math.max(10, Number(draft.backupHistoryLimit) || 200),
        backupTimeoutSecs: Math.max(60, Number(draft.backupTimeoutSecs) || 3600),
        cloudflareApiToken: draft.cloudflareApiToken.trim(),
        cloudflareAccountId: draft.cloudflareAccountId.trim(),
        pagesHistoryLimit: Math.max(10, Number(draft.pagesHistoryLimit) || 200),
      });
      setSettings(saved);
      toast("success", "设置已保存");
    } catch (error) {
      toast("error", String(error));
    }
  }

  async function handleSaveTargets() {
    try {
      const saved = await api.saveBackupTargets(targetDraft);
      setTargetDraft(saved);
      await Promise.all([refreshBackupTargets(), refreshServers()]);
      // 后端可能清理指向已删除目标的默认设置，需要同步最新 settings。
      const fresh = await api.getSettings();
      setSettings(fresh);
      toast("success", "备份目标已保存");
    } catch (error) {
      toast("error", String(error));
    }
  }

  /** 把旧版单连接串转成正式备份目标，并立即持久化，避免只改本地状态导致数据丢失。 */
  async function handleConvertLegacy(url: string) {
    const next = [...targetDraft, { id: "", name: "旧版连接串", url }];
    try {
      const saved = await api.saveBackupTargets(next);
      setTargetDraft(saved);
      await refreshBackupTargets();
      const savedSettings = await api.saveSettings({ ...draft, supabaseUrl: "" });
      setSettings(savedSettings);
      toast("success", "已将旧连接串转为备份目标");
    } catch (error) {
      toast("error", String(error));
    }
  }

  return (
    <Page title="设置" subtitle="全局参数与数据目录">
      <div className="flex max-w-4xl flex-col gap-8">
        <section>
          <SectionTitle
            title="部署参数"
            description="打包、上传与脚本执行相关配置"
            actions={
              <Button
                icon={<Settings2 className="size-4" />}
                onClick={() => void handleSaveSettings()}
              >
                保存设置
              </Button>
            }
          />
          <Card className="p-5">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <Field label="默认脚本目录" hint="相对项目根目录">
                <Input
                  value={draft.scriptDir}
                  onChange={(event) => setDraft({ ...draft, scriptDir: event.target.value })}
                  placeholder="docker"
                />
              </Field>
              <Field label="部署记录保留条数">
                <Input
                  type="number"
                  value={draft.historyLimit}
                  onChange={(event) =>
                    setDraft({ ...draft, historyLimit: Number(event.target.value) })
                  }
                />
              </Field>
              <Field label="SSH 连接超时（秒）">
                <Input
                  type="number"
                  value={draft.connectTimeoutSecs}
                  onChange={(event) =>
                    setDraft({ ...draft, connectTimeoutSecs: Number(event.target.value) })
                  }
                />
              </Field>
              <Field label="脚本执行超时（秒）">
                <Input
                  type="number"
                  value={draft.scriptTimeoutSecs}
                  onChange={(event) =>
                    setDraft({ ...draft, scriptTimeoutSecs: Number(event.target.value) })
                  }
                />
              </Field>
            </div>

            <div className="mt-5 flex flex-col gap-3 border-t border-line pt-5">
              <label className="flex items-center gap-2.5 text-xs text-ink">
                <input
                  type="checkbox"
                  checked={draft.runScripts}
                  onChange={(event) => setDraft({ ...draft, runScripts: event.target.checked })}
                  className="size-3.5 accent-primary"
                />
                默认开启「上传解压后执行项目脚本」
              </label>
              <label className="flex items-center gap-2.5 text-xs text-ink">
                <input
                  type="checkbox"
                  checked={draft.keepRemoteArchive}
                  onChange={(event) =>
                    setDraft({ ...draft, keepRemoteArchive: event.target.checked })
                  }
                  className="size-3.5 accent-primary"
                />
                在服务器上保留上传的代码包（默认解压后删除）
              </label>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title="数据库备份目标"
            description="Supabase / Aiven / Neon 等 PostgreSQL，可配置多个，备份时选择一个"
            actions={
              <Button
                icon={<Save className="size-4" />}
                onClick={() => void handleSaveTargets()}
              >
                保存目标
              </Button>
            }
          />
          <Card className="flex flex-col gap-3 p-5">
            {targetDraft.length === 0 && (
              <p className="text-xs text-ink-faint">还没有备份目标，点击下方「添加目标」。</p>
            )}
            {targetDraft.map((target, index) => (
              <div
                key={target.id || `new-${index}`}
                className="grid grid-cols-[150px_minmax(0,1fr)_auto] items-center gap-2"
              >
                <Input
                  value={target.name}
                  placeholder="名称（如 Supabase）"
                  onChange={(event) => {
                    const next = [...targetDraft];
                    next[index] = { ...target, name: event.target.value };
                    setTargetDraft(next);
                  }}
                />
                <Input
                  value={target.url}
                  placeholder="postgresql://user:password@host:5432/postgres"
                  onChange={(event) => {
                    const next = [...targetDraft];
                    next[index] = { ...target, url: event.target.value };
                    setTargetDraft(next);
                  }}
                />
                <Button
                  variant="ghost"
                  size="sm"
                  title="删除目标"
                  onClick={() => setTargetDraft(targetDraft.filter((_, i) => i !== index))}
                >
                  <Trash2 className="size-3.5" />
                </Button>
              </div>
            ))}
            <div className="flex items-center gap-2 border-t border-line pt-3">
              <Button
                variant="secondary"
                size="sm"
                icon={<Plus className="size-3.5" />}
                onClick={() =>
                  setTargetDraft([
                    ...targetDraft,
                    { id: "", name: "", url: "" },
                  ])
                }
              >
                添加目标
              </Button>
            </div>

            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <Field label="全局默认目标" hint="服务器未指定时使用">
                <Select
                  value={draft.defaultBackupTargetId ?? ""}
                  onChange={(event) =>
                    setDraft({
                      ...draft,
                      defaultBackupTargetId: event.target.value || null,
                    })
                  }
                >
                  <option value="">无（不备份）</option>
                  {backupTargets.map((target) => (
                    <option key={target.id} value={target.id}>
                      {target.name}
                    </option>
                  ))}
                </Select>
              </Field>
              <Field label="备份记录保留条数">
                <Input
                  type="number"
                  value={draft.backupHistoryLimit}
                  onChange={(event) =>
                    setDraft({ ...draft, backupHistoryLimit: Number(event.target.value) })
                  }
                />
              </Field>
              <Field label="备份超时（秒）">
                <Input
                  type="number"
                  value={draft.backupTimeoutSecs}
                  onChange={(event) =>
                    setDraft({ ...draft, backupTimeoutSecs: Number(event.target.value) })
                  }
                />
              </Field>
            </div>

            <div className="flex items-center justify-between gap-3 border-t border-line pt-3">
              {draft.supabaseUrl.trim() ? (
                <div className="flex min-w-0 items-center gap-2 text-[11px] text-warn">
                  <span className="min-w-0">
                    检测到旧版单连接串配置，将作为最低优先级兜底。
                  </span>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void handleConvertLegacy(draft.supabaseUrl.trim())}
                  >
                    转为备份目标
                  </Button>
                </div>
              ) : (
                <span className="text-[11px] text-ink-faint">
                  全量覆盖：执行时清空目标 schema 后重新导入，并恢复默认角色授权
                </span>
              )}
              <Button
                variant="secondary"
                size="sm"
                icon={<Save className="size-3.5" />}
                onClick={() => void handleSaveSettings()}
              >
                保存备份参数
              </Button>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle
            title="Cloudflare Pages"
            description="用于「构建并部署」到 Cloudflare Pages，Token 需 Pages:Edit 权限"
          />
          <Card className="p-5">
            <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
              <Field label="API Token" hint="Cloudflare 控制台创建">
                <Input
                  type="password"
                  value={draft.cloudflareApiToken}
                  onChange={(event) =>
                    setDraft({ ...draft, cloudflareApiToken: event.target.value })
                  }
                  placeholder="Cloudflare API Token"
                />
              </Field>
              <Field label="Account ID">
                <Input
                  value={draft.cloudflareAccountId}
                  onChange={(event) =>
                    setDraft({ ...draft, cloudflareAccountId: event.target.value })
                  }
                  placeholder="Cloudflare Account ID"
                />
              </Field>
              <Field label="Pages 记录保留条数">
                <Input
                  type="number"
                  value={draft.pagesHistoryLimit}
                  onChange={(event) =>
                    setDraft({ ...draft, pagesHistoryLimit: Number(event.target.value) })
                  }
                />
              </Field>
            </div>
            <div className="mt-4 flex justify-end border-t border-line pt-4">
              <Button
                variant="secondary"
                size="sm"
                icon={<Save className="size-3.5" />}
                onClick={() => void handleSaveSettings()}
              >
                保存 Cloudflare 配置
              </Button>
            </div>
          </Card>
        </section>

        <section>
          <SectionTitle title="外观" description="界面主题，「系统」跟随操作系统深浅色自动切换" />
          <Card className="flex items-center gap-4 p-4">
            <span className="min-w-0 flex-1 text-[13px] text-ink-dim">界面主题</span>
            <ThemeToggle className="w-72 shrink-0" />
          </Card>
        </section>

        <section>
          <SectionTitle title="数据目录" description="配置、部署记录与临时文件存放位置" />
          <Card className="flex items-center gap-3 p-5">
            <p className="min-w-0 flex-1 truncate font-mono text-xs text-ink-dim" title={dataDir}>
              {dataDir || "读取中 ..."}
            </p>
            <Button
              variant="secondary"
              icon={<FolderOpen className="size-4" />}
              disabled={!dataDir}
              onClick={() => void api.revealPath(dataDir).catch((e) => toast("error", String(e)))}
            >
              打开目录
            </Button>
          </Card>
        </section>
      </div>
    </Page>
  );
}
