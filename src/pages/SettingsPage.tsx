import { useEffect, useState } from "react";
import { FolderOpen, Settings2 } from "lucide-react";

import { Button, Card, Field, Input, Page, SectionTitle } from "../components/ui";
import { ThemeToggle } from "../components/ThemeToggle";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { Settings } from "../lib/types";

export default function SettingsPage() {
  const settings = useApp((state) => state.settings);
  const setSettings = useApp((state) => state.setSettings);
  const toast = useApp((state) => state.toast);

  const [dataDir, setDataDir] = useState("");
  const [draft, setDraft] = useState<Settings>(settings);

  useEffect(() => {
    void api
      .getDataDir()
      .then(setDataDir)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    setDraft(settings);
  }, [settings]);

  async function handleSaveSettings() {
    try {
      const saved = await api.saveSettings({
        ...draft,
        scriptDir: draft.scriptDir.trim() || "docker",
        connectTimeoutSecs: Math.max(3, Number(draft.connectTimeoutSecs) || 15),
        scriptTimeoutSecs: Math.max(10, Number(draft.scriptTimeoutSecs) || 1800),
        historyLimit: Math.max(20, Number(draft.historyLimit) || 500),
      });
      setSettings(saved);
      toast("success", "设置已保存");
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
