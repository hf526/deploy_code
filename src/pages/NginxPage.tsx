import { useCallback, useEffect, useRef, useState } from "react";
import { FileCode2, Pencil, Plus, RefreshCw, RotateCw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import {
  Button,
  Card,
  ConfirmModal,
  EmptyState,
  Field,
  Input,
  Page,
  SectionTitle,
  Select,
} from "../components/ui";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import type { NginxConfigFile, NginxContainerInfo } from "../lib/types";
import { humanSize } from "../lib/utils";
import { NginxConfigModal, type NginxEditorState } from "./nginx/NginxConfigModal";
import { defaultNginxTemplate } from "./nginx/template";

const DEFAULT_CONFIG_DIR = "/etc/nginx/conf.d";

/** 删除确认的目标快照：确认期间切换服务器/容器也不会删错目标。 */
interface NginxDeleteTarget {
  name: string;
  serverId: string;
  container: string;
  dir: string;
}

export default function NginxPage() {
  const { t } = useTranslation();
  const servers = useApp((state) => state.servers);
  const toast = useApp((state) => state.toast);

  const [serverId, setServerId] = useState("");
  const [containers, setContainers] = useState<NginxContainerInfo[]>([]);
  const [container, setContainer] = useState("");
  const [dir, setDir] = useState(DEFAULT_CONFIG_DIR);
  const [filesDir, setFilesDir] = useState(DEFAULT_CONFIG_DIR);
  const [files, setFiles] = useState<NginxConfigFile[]>([]);
  const [loadingContainers, setLoadingContainers] = useState(false);
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [loadingName, setLoadingName] = useState("");
  const [editing, setEditing] = useState<NginxEditorState | null>(null);
  const [deleting, setDeleting] = useState<NginxDeleteTarget | null>(null);
  const [deletingBusy, setDeletingBusy] = useState(false);
  const [reloading, setReloading] = useState(false);

  // 查询序号：切换服务器/容器/目录后，旧请求返回时直接丢弃，避免串数据。
  const querySeq = useRef(0);
  const dirRef = useRef(dir);
  const containerRef = useRef(container);
  // 最新上下文快照：异步回调里不能用闭包捕获的 state 比较（保存/删除期间可能已切换）。
  const contextRef = useRef({ serverId, container });

  useEffect(() => {
    dirRef.current = dir;
  }, [dir]);

  useEffect(() => {
    containerRef.current = container;
  }, [container]);

  useEffect(() => {
    contextRef.current = { serverId, container };
  }, [serverId, container]);

  useEffect(() => {
    if (servers.length === 0) {
      setServerId("");
      return;
    }
    if (!servers.some((server) => server.id === serverId)) {
      setServerId(servers[0].id);
    }
  }, [servers, serverId]);

  const loadFiles = useCallback(
    async (targetServer: string, targetContainer: string, targetDir: string) => {
      const seq = ++querySeq.current;
      setLoadingFiles(true);
      try {
        const list = await api.listNginxConfigs(targetServer, targetContainer, targetDir);
        if (seq !== querySeq.current) return;
        setFiles(list);
        setFilesDir(targetDir);
      } catch (error) {
        if (seq !== querySeq.current) return;
        setFiles([]);
        toast("error", String(error));
      } finally {
        if (seq === querySeq.current) setLoadingFiles(false);
      }
    },
    [toast],
  );

  const loadContainers = useCallback(
    async (targetServer: string): Promise<string> => {
      const seq = ++querySeq.current;
      setLoadingContainers(true);
      try {
        const list = await api.listNginxContainers(targetServer);
        if (seq !== querySeq.current) return "";
        setContainers(list);
        // 保留用户当前选择；否则优先 nginx 容器。不自动选非 nginx 容器，
        // 避免首次进入页面就对着 postgres 之类容器查询 conf.d 弹出错误。
        const current = containerRef.current;
        const preferred =
          list.find((item) => item.name === current) ??
          list.find((item) => item.nginx) ??
          null;
        setContainer(preferred?.name ?? "");
        return preferred?.name ?? "";
      } catch (error) {
        if (seq !== querySeq.current) return "";
        setContainers([]);
        setContainer("");
        toast("error", String(error));
        return "";
      } finally {
        if (seq === querySeq.current) setLoadingContainers(false);
      }
    },
    [toast],
  );

  // 服务器变化：清空上下文并重新查询容器。
  useEffect(() => {
    querySeq.current += 1;
    setContainers([]);
    setContainer("");
    setFiles([]);
    // 同步重置 ref：避免新服务器恰好有同名容器时把旧选择带过去。
    containerRef.current = "";
    if (serverId) void loadContainers(serverId);
  }, [serverId, loadContainers]);

  // 容器变化：按当前配置目录查询配置文件（目录输入框改动不即时查询）。
  useEffect(() => {
    if (!serverId || !container) {
      // 容器被清空（无容器 / 查询失败）时不能继续展示上一个容器的文件。
      setFiles([]);
      return;
    }
    void loadFiles(serverId, container, dirRef.current);
  }, [serverId, container, loadFiles]);

  async function handleRefreshContainers() {
    if (!serverId) return;
    const preferred = await loadContainers(serverId);
    // 容器名未变化时上面的 effect 不会触发，这里补一次配置查询。
    // 用 ref 比较：等待期间用户可能已手动切换容器，不能再用旧值触发加载。
    if (preferred && preferred === containerRef.current) {
      void loadFiles(serverId, preferred, dirRef.current);
    }
  }

  function handleQuery() {
    if (!serverId || !container) return;
    void loadFiles(serverId, container, dir);
  }

  async function handleEdit(file: NginxConfigFile) {
    if (!serverId || !container || loadingName) return;
    const target = { serverId, container, dir: filesDir };
    const seq = querySeq.current;
    setLoadingName(file.name);
    try {
      const detail = await api.readNginxConfig(
        target.serverId,
        target.container,
        target.dir,
        file.name,
      );
      // 读取期间切换了服务器/容器：丢弃结果，避免旧内容被保存到新目标。
      if (seq !== querySeq.current) return;
      setEditing({ name: detail.name, content: detail.content, isNew: false, ...target });
    } catch (error) {
      toast("error", String(error));
    } finally {
      setLoadingName("");
    }
  }

  function handleNew() {
    if (!serverId || !container) return;
    setEditing({
      name: "",
      content: defaultNginxTemplate(),
      isNew: true,
      serverId,
      container,
      dir: filesDir,
    });
  }

  async function handleDelete() {
    if (!deleting || deletingBusy) return;
    const target = deleting;
    const seq = querySeq.current;
    setDeletingBusy(true);
    try {
      const message = await api.deleteNginxConfig(
        target.serverId,
        target.container,
        target.dir,
        target.name,
      );
      toast("success", message);
      setDeleting(null);
      // 删除期间切换了服务器/容器：让新上下文的加载结果生效，不要用旧目录覆盖。
      if (seq === querySeq.current) {
        await loadFiles(target.serverId, target.container, target.dir);
      }
    } catch (error) {
      toast("error", String(error));
    } finally {
      setDeletingBusy(false);
    }
  }

  async function handleReload() {
    if (!serverId || !container || reloading) return;
    setReloading(true);
    try {
      const message = await api.reloadNginx(serverId, container);
      toast("success", message);
    } catch (error) {
      toast("error", String(error));
    } finally {
      setReloading(false);
    }
  }

  const hasTarget = Boolean(serverId && container);

  return (
    <Page
      title={t("nav.nginx")}
      subtitle={t("nginx.subtitle")}
      actions={
        <Button
          variant="secondary"
          icon={<RotateCw className="size-4" />}
          loading={reloading}
          disabled={!hasTarget}
          onClick={() => void handleReload()}
        >
          {t("nginx.reload")}
        </Button>
      }
    >
      <div className="flex flex-col gap-6">
        <section>
          <SectionTitle
            title={t("nginx.connection")}
            description={t("nginx.connectionDescription")}
          />
          <Card className="grid grid-cols-1 gap-4 p-5 md:grid-cols-3">
            <Field label={t("nginx.server")} required>
              <Select
                value={serverId}
                disabled={servers.length === 0}
                onChange={(event) => {
                  // 立即作废在途查询，避免旧服务器的响应在新上下文里落地。
                  querySeq.current += 1;
                  setServerId(event.target.value);
                }}
              >
                {servers.length === 0 && <option value="">{t("nginx.noServers")}</option>}
                {servers.map((server) => (
                  <option key={server.id} value={server.id}>
                    {server.name} · {server.host}
                  </option>
                ))}
              </Select>
            </Field>

            <Field label={t("nginx.container")} required hint={t("nginx.containerHint")}>
              <div className="flex gap-2">
                <Select
                  value={container}
                  disabled={loadingContainers || containers.length === 0}
                  onChange={(event) => {
                    // 立即清空旧列表并作废在途查询，避免新容器选择期间闪回旧数据。
                    querySeq.current += 1;
                    setFiles([]);
                    setContainer(event.target.value);
                  }}
                >
                  {containers.length === 0 ? (
                    <option value="">{t("nginx.noContainers")}</option>
                  ) : (
                    <>
                      <option value="">{t("common.selectPlaceholder")}</option>
                      {containers.map((item) => (
                        <option key={item.name} value={item.name}>
                          {item.name} · {item.image}
                          {item.nginx ? "" : ` (${t("nginx.notNginx")})`}
                        </option>
                      ))}
                    </>
                  )}
                </Select>
                <Button
                  variant="secondary"
                  title={t("nginx.refreshContainers")}
                  loading={loadingContainers}
                  disabled={!serverId}
                  onClick={() => void handleRefreshContainers()}
                >
                  <RefreshCw className="size-4" />
                </Button>
              </div>
            </Field>

            <Field label={t("nginx.configDir")}>
              <div className="flex gap-2">
                <Input
                  value={dir}
                  disabled={!serverId}
                  placeholder={DEFAULT_CONFIG_DIR}
                  onChange={(event) => setDir(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") handleQuery();
                  }}
                />
                <Button variant="secondary" loading={loadingFiles} disabled={!hasTarget} onClick={handleQuery}>
                  {t("nginx.query")}
                </Button>
              </div>
            </Field>
          </Card>
        </section>

        <section>
          <SectionTitle
            title={t("nginx.configSection")}
            description={
              hasTarget ? `${container} · ${filesDir}` : t("nginx.configSectionHint")
            }
            actions={
              <div className="flex gap-2">
                <Button
                  size="sm"
                  variant="secondary"
                  icon={<RefreshCw className="size-3.5" />}
                  loading={loadingFiles}
                  disabled={!hasTarget}
                  onClick={handleQuery}
                >
                  {t("common.refresh")}
                </Button>
                <Button
                  size="sm"
                  icon={<Plus className="size-3.5" />}
                  disabled={!hasTarget}
                  onClick={handleNew}
                >
                  {t("nginx.newConfig")}
                </Button>
              </div>
            }
          />
          {files.length === 0 ? (
            <EmptyState
              icon={<FileCode2 className="size-4.5" />}
              title={t("nginx.empty")}
              description={t("nginx.emptyDescription")}
            />
          ) : (
            <Card className="divide-y divide-line overflow-hidden">
              {files.map((file) => (
                <div key={file.name} className="flex items-center gap-3 px-4 py-3">
                  <span className="grid size-8 shrink-0 place-items-center rounded-md border border-brand-line bg-brand-soft text-brand">
                    <FileCode2 className="size-4" />
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="truncate font-mono text-[13px] font-medium text-ink">{file.name}</p>
                    <p className="mt-0.5 text-[11px] text-ink-faint">{humanSize(file.size)}</p>
                  </div>
                  <Button
                    variant="secondary"
                    size="sm"
                    title={t("nginx.edit")}
                    loading={loadingName === file.name}
                    disabled={loadingFiles || Boolean(loadingName)}
                    onClick={() => void handleEdit(file)}
                  >
                    <Pencil className="size-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    title={t("nginx.delete")}
                    disabled={loadingFiles || Boolean(loadingName)}
                    onClick={() =>
                      setDeleting({ name: file.name, serverId, container, dir: filesDir })
                    }
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </div>
              ))}
            </Card>
          )}
        </section>
      </div>

      {editing && (
        <NginxConfigModal
          state={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            const target = editing;
            // 保存期间用户可能已关掉弹窗并打开另一个文件：只关闭仍指向本次目标的那一个。
            setEditing((current) => (current === target ? null : current));
            // 保存目标是打开弹窗时的快照；与最新上下文比较，避免用旧目录覆盖新列表。
            const context = contextRef.current;
            if (target.serverId === context.serverId && target.container === context.container) {
              void loadFiles(target.serverId, target.container, target.dir);
            }
          }}
        />
      )}

      <ConfirmModal
        open={deleting !== null}
        danger
        loading={deletingBusy}
        title={t("nginx.deleteTitle")}
        confirmText={t("common.delete")}
        description={t("nginx.deleteDescription", { name: deleting?.name ?? "" })}
        onCancel={() => setDeleting(null)}
        onConfirm={() => void handleDelete()}
      />
    </Page>
  );
}
