import { useCallback, useEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { Ban, Loader2, LogOut, RefreshCw, X } from "lucide-react";

import { Badge, Button, ConfirmModal, Modal } from "../../components/ui";
import { api } from "../../lib/api";
import i18n from "../../lib/i18n";
import { blockGuardReason, isWhitelisted, type BlockGuardReason } from "../../lib/security";
import { useApp } from "../../lib/store";
import { GuardSection } from "./GuardSection";
import type { OnlineSession, SecurityReport, SecuritySetting, ServerConfig } from "../../lib/types";

function firewallLabel(firewall: string): string {
  if (firewall === "none") return i18n.t("servers.firewallNone");
  if (firewall === "unknown") return i18n.t("servers.firewallUnknown");
  if (firewall === "ufw") return "UFW";
  return firewall;
}

function settingTone(setting: SecuritySetting): "green" | "red" | "amber" | "gray" {
  if (setting.key === "permitrootlogin") return setting.value.startsWith("yes") ? "red" : "green";
  if (setting.key === "passwordauthentication") {
    return setting.value.startsWith("yes") ? "amber" : "green";
  }
  return "gray";
}

/** 保护理由的完整说明（挂在旁边的短标签上，鼠标悬停看细节）。 */
function blockReasonTip(reason: BlockGuardReason, ip: string): string {
  return reason === "self"
    ? i18n.t("servers.blockReasonSelf", { ip })
    : i18n.t("servers.blockReasonWhitelisted", { ip });
}

/**
 * 安全扫描弹窗：扫描、拉黑、解除、踢会话都在这一条 SSH 会话上跑。
 *
 * 序号 + 存活标记是原来 ServersPage 里那套竞态守卫搬过来的：关窗后旧请求的结果不能
 * 再落进界面，否则 A 的报告会显示到 B 上，或对错误的服务器执行拉黑。调用方按
 * `key={server.id}` 挂载，换一台就是换实例。
 */
export function SecurityModal({
  server,
  onClose,
}: {
  server: ServerConfig;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const toast = useApp((state) => state.toast);
  const [report, setReport] = useState<SecurityReport | null>(null);
  const [scanning, setScanning] = useState(false);
  const [secError, setSecError] = useState<string | null>(null);
  const [ipBusy, setIpBusy] = useState<string | null>(null);
  const [kicking, setKicking] = useState<OnlineSession | null>(null);
  const [sessionBusy, setSessionBusy] = useState<string | null>(null);

  const scanSeq = useRef(0);
  const alive = useRef(true);

  const scan = useCallback(async () => {
    const seq = ++scanSeq.current;
    setScanning(true);
    try {
      const result = await api.scanServerSecurity(server);
      if (!alive.current || seq !== scanSeq.current) return;
      setReport(result);
      setSecError(null);
    } catch (error) {
      if (!alive.current || seq !== scanSeq.current) return;
      setSecError(String(error));
    } finally {
      if (alive.current && seq === scanSeq.current) setScanning(false);
    }
  }, [server]);

  useEffect(() => {
    alive.current = true;
    void scan();
    return () => {
      alive.current = false;
      // 让在途的那一次结果作废。
      scanSeq.current += 1;
    };
  }, [scan]);

  async function handleBlock(ip: string) {
    setIpBusy(ip);
    try {
      const message = await api.blockServerIp(server, ip);
      toast("success", message);
      await scan();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setIpBusy(null);
    }
  }

  async function handleUnblock(ip: string) {
    setIpBusy(ip);
    try {
      const message = await api.unblockServerIp(server, ip);
      toast("success", message);
      await scan();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setIpBusy(null);
    }
  }

  async function handleKick() {
    if (!kicking) return;
    setSessionBusy(kicking.tty);
    try {
      const message = await api.kickServerSession(server, kicking.tty);
      toast("success", message);
      setKicking(null);
      await scan();
    } catch (error) {
      toast("error", String(error));
    } finally {
      setSessionBusy(null);
    }
  }

  return (
    <>
      <Modal
        open
        onClose={onClose}
        title={t("servers.securityTitle")}
        subtitle={`${server.name} · ${server.username}@${server.host}:${server.port}`}
        width="max-w-3xl"
        footer={
          <>
            <Button variant="secondary" onClick={onClose}>
              {t("common.close")}
            </Button>
            <Button
              variant="secondary"
              loading={scanning}
              icon={<RefreshCw className="size-4" />}
              onClick={() => void scan()}
            >
              {t("servers.rescan")}
            </Button>
          </>
        }
      >
        {scanning && !report ? (
          <p className="flex items-center justify-center gap-2 py-12 text-xs text-ink-dim">
            <Loader2 className="size-4 animate-spin" /> {t("servers.scanning")}
          </p>
        ) : secError ? (
          <p className="rounded-md border border-neg/30 bg-neg-soft px-4 py-3 text-xs break-all text-neg">
            {secError}
          </p>
        ) : report ? (
          <div className="flex flex-col gap-5">
            <div className="flex flex-wrap items-center gap-2">
              <Badge
                kind={
                  report.firewall === "none" ? "red" : report.firewall === "unknown" ? "amber" : "brand"
                }
              >
                {t("servers.firewall", { value: firewallLabel(report.firewall) })}
              </Badge>
              <Badge kind={report.isRoot || report.hasSudo ? "green" : "amber"}>
                {report.isRoot
                  ? t("servers.privilegeRoot")
                  : report.hasSudo
                    ? t("servers.privilegeSudo")
                    : t("servers.privilegeLimited")}
              </Badge>
            </div>

            {report.notes.length > 0 && (
              <div className="rounded-md border border-warn/30 bg-warn-soft px-3.5 py-2.5 text-[11.5px] leading-relaxed text-warn">
                {report.notes.map((note) => (
                  <p key={note}>{note}</p>
                ))}
              </div>
            )}

            <div className="flex flex-wrap items-center gap-2 text-[11px]">
              <span className="rounded-md border border-line bg-field px-2.5 py-1 text-ink-dim">
                {t("servers.onlineSessions")}{" "}
                <b className="ml-1 text-ink">{report.sessions.length}</b>
              </span>
              <span className="rounded-md border border-line bg-field px-2.5 py-1 text-ink-dim">
                {t("servers.lastScan")}{" "}
                <b className="ml-1 text-ink">{report.scannedAt || "-"}</b>
              </span>
            </div>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">
                {t("servers.failedLogins", { count: report.failed.length })}
              </h4>
              {report.failed.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noFailedLogins")}</p>
              ) : (
                <ul className="divide-y divide-line overflow-hidden rounded-md border border-line">
                  {report.failed.map((item) => {
                    // 判定用服务器上的白名单：界面上没点「更新」的改动还没生效，远端只认那份配置。
                    // 禁用态的按钮不吃指针事件（ui.tsx 的 disabled:pointer-events-none），
                    // 所以理由必须摆在旁边，挂 title 是看不到的。
                    const reason = blockGuardReason(report.whitelist, report.selfIp, item.ip);
                    return (
                      <li
                        key={`${item.user}-${item.ip}`}
                        className="flex items-center gap-3 px-3 py-1.5 transition-colors hover:bg-hover"
                      >
                        <span
                          className="w-40 shrink-0 truncate font-mono text-xs text-ink"
                          title={item.user}
                        >
                          {item.user}
                        </span>
                        <span className="min-w-0 flex-1 truncate font-mono text-xs text-ink-dim">
                          {item.ip}
                        </span>
                        <Badge kind="red">{t("servers.times", { count: item.count })}</Badge>
                        {report.blocked.includes(item.ip) ? (
                          <Button
                            size="sm"
                            variant="secondary"
                            loading={ipBusy === item.ip}
                            disabled={ipBusy !== null && ipBusy !== item.ip}
                            icon={<X className="size-3.5" />}
                            onClick={() => void handleUnblock(item.ip)}
                          >
                            {t("servers.unblock")}
                          </Button>
                        ) : (
                          <>
                            {reason && (
                              <span title={blockReasonTip(reason, item.ip)} className="text-[11px] text-warn">
                                {reason === "self"
                                  ? t("servers.blockGuardSelf")
                                  : t("servers.guardWhitelistBadge")}
                              </span>
                            )}
                            <Button
                              size="sm"
                              variant="secondary"
                              loading={ipBusy === item.ip}
                              disabled={(ipBusy !== null && ipBusy !== item.ip) || reason !== null}
                              icon={<Ban className="size-3.5" />}
                              onClick={() => void handleBlock(item.ip)}
                            >
                              {t("servers.block")}
                            </Button>
                          </>
                        )}
                      </li>
                    );
                  })}
                </ul>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.recentSuccess")}</h4>
              {report.success.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noSuccess")}</p>
              ) : (
                <ul className="divide-y divide-line overflow-hidden rounded-md border border-line">
                  {report.success.map((item, index) => (
                    <li
                      key={`${item.user}-${item.ip}-${index}`}
                      className="flex items-center gap-3 px-3 py-1.5"
                    >
                      <span className="w-28 shrink-0 truncate font-mono text-xs text-ink">
                        {item.user}
                      </span>
                      <span className="w-40 shrink-0 truncate font-mono text-xs text-ink-dim">
                        {item.ip}
                      </span>
                      <span
                        className="min-w-0 flex-1 truncate text-[11px] text-ink-faint"
                        title={item.detail}
                      >
                        {item.detail}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">
                {t("servers.sessionsTitle", { count: report.sessions.length })}
              </h4>
              {report.sessions.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noSessions")}</p>
              ) : (
                <ul className="divide-y divide-line overflow-hidden rounded-md border border-line">
                  {report.sessions.map((session) => (
                    <li
                      key={`${session.user}-${session.tty}`}
                      className="flex items-center gap-3 px-3 py-1.5 transition-colors hover:bg-hover"
                    >
                      <span
                        className="w-28 shrink-0 truncate font-mono text-xs text-ink"
                        title={session.user}
                      >
                        {session.user}
                      </span>
                      <span className="w-20 shrink-0 truncate font-mono text-xs text-ink-dim">
                        {session.tty}
                      </span>
                      <span
                        className="min-w-0 flex-1 truncate text-[11px] text-ink-faint"
                        title={`${session.loginAt}${session.from ? ` · ${session.from}` : ""}`}
                      >
                        {session.loginAt}
                        {session.from ? ` · ${session.from}` : ""}
                      </span>
                      <Button
                        size="sm"
                        variant="secondary"
                        loading={sessionBusy === session.tty}
                        disabled={sessionBusy !== null && sessionBusy !== session.tty}
                        icon={<LogOut className="size-3.5" />}
                        onClick={() => setKicking(session)}
                      >
                        {t("servers.kick")}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </section>

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.blockedIps")}</h4>
              {report.blocked.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noBlockedIps")}</p>
              ) : (
                <div className="flex flex-wrap gap-2">
                  {report.blocked.map((ip) => {
                    // 白名单里的地址还挂在拦截列表里 = 自己（或定时任务）正被挡在外面，
                    // 白名单只挡以后的拉黑、不会自动把已封的放出来，所以单独标出来。
                    const guarded = isWhitelisted(report.whitelist, ip);
                    return (
                      <span
                        key={ip}
                        title={guarded ? t("servers.blockedWhitelisted") : undefined}
                        className={`inline-flex items-center gap-1.5 rounded-md border py-0.5 pr-1 pl-2 font-mono text-[11px] ${
                          guarded ? "border-warn/30 bg-warn-soft text-warn" : "border-line bg-field text-ink"
                        }`}
                      >
                        {ip}
                        {guarded && (
                          <span className="font-sans text-[10px]">{t("servers.guardWhitelistBadge")}</span>
                        )}
                        <button
                          type="button"
                          disabled={ipBusy !== null}
                          onClick={() => void handleUnblock(ip)}
                          className="rounded px-1.5 py-0.5 text-[10px] text-ink-faint hover:bg-hover hover:text-ink disabled:opacity-50"
                        >
                          {ipBusy === ip ? t("servers.unblocking") : t("servers.unblock")}
                        </button>
                      </span>
                    );
                  })}
                </div>
              )}
            </section>

            <GuardSection server={server} report={report} onChanged={() => void scan()} />

            <section>
              <h4 className="mb-2 text-xs font-semibold text-ink">{t("servers.sshdTitle")}</h4>
              {report.sshd.length === 0 ? (
                <p className="text-xs text-ink-faint">{t("servers.noSshd")}</p>
              ) : (
                <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
                  {report.sshd.map((setting) => (
                    <div
                      key={setting.key}
                      className="flex items-center justify-between gap-2 rounded-md border border-line bg-field px-3 py-1.5"
                    >
                      <span className="truncate font-mono text-[11px] text-ink-dim">
                        {setting.key}
                      </span>
                      <Badge kind={settingTone(setting)}>{setting.value}</Badge>
                    </div>
                  ))}
                </div>
              )}
            </section>
          </div>
        ) : null}
      </Modal>

      <ConfirmModal
        open={!!kicking}
        danger
        loading={sessionBusy !== null}
        title={t("servers.kickTitle")}
        confirmText={t("servers.kick")}
        description={
          <Trans
            i18nKey={kicking?.from ? "servers.kickDescriptionWithFrom" : "servers.kickDescription"}
            values={{ user: kicking?.user, tty: kicking?.tty, from: kicking?.from }}
            components={{ b: <b className="text-ink" /> }}
          />
        }
        onCancel={() => setKicking(null)}
        onConfirm={() => void handleKick()}
      />
    </>
  );
}
