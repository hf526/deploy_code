//! 每台服务器一条常驻 SSH 会话：隧道转发与主机资源探针都挂在同一条会话上。
//!
//! 「常驻」是把最贵的那一项买断：认证次数。按次采集的话，每刷新一次每台都要走完
//! 一遍 TCP + 密钥交换 + 认证；改成一活多久的会话之后，认证只在真正断线时发生，
//! 每一轮采集只是在既有会话上多开一条通道。顺带把「高频连同一台机器」这件事可能
//! 触发本项目自带封禁守护（`security.rs` 按窗口阈值拉黑来源 IP）的风险也消掉了。
//!
//! 一台服务器一个 [`supervise`] 任务：绑定本机端口 -> 建立会话 -> 转发 + 探针直到
//! 会话不可用 -> 退避重来。会话断了只重连、不退出，所以笔记本合盖唤醒、服务器重启
//! 之后都会自己回来。首轮连接与每轮退避都按服务器 id 错开几秒：合盖唤醒时所有会话
//! 同时判死，挤在一起重连会把认证失败堆进同一分钟里。
//!
//! 所有已登记的服务器都建会话，不只是配了隧道规则的——探针要的是每一台。端口一个
//! 没抢到也不再跳过建连（那是老的一次性采集留下的分支）：转发这一轮可以没得转，
//! 会话本身还得起来。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::error::{CoreError, Result};
use crate::metrics::{self, HostMetrics};
use crate::models::{now_string, ServerConfig, TunnelRule};
use crate::ssh::SshClient;
use crate::tunnel::{fingerprint, TunnelRuleStatus};

/// 建立 SSH 连接的超时（含认证）。会话是常驻的，这里不必太长。
const CONNECT_TIMEOUT_SECS: u64 = 15;
/// 重连退避的起止与「会话算稳定」的门槛：秒断的服务器不会把间隔顶到最大。
const RECONNECT_MIN_SECS: u64 = 3;
const RECONNECT_MAX_SECS: u64 = 60;
const STABLE_SESSION_SECS: u64 = 60;
/// 会话存活探测间隔：russh 的句柄在会话任务结束后 `is_closed()` 即为真。
const HEALTH_CHECK_SECS: u64 = 2;
/// 退出前等在途转发任务收口的最长时间，超时直接 abort。
const DRAIN_TIMEOUT_SECS: u64 = 2;
/// 本机 accept 失败后回到 accept 之前的间隔：只在连续失败时挡空转，正常连接感觉不到。
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(200);
/// 两次采集之间的间隔。一次采集自带一秒钟的采样窗口，间隔只要大于窗口就够用。
const PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// 一次采集的预算，由这一层掐表。到点不记错了继续跑，而是换掉整条会话——
/// 见 [`metrics::SAMPLE_CEILING_SECS`] 那一段。
const SAMPLE_DEADLINE: Duration = Duration::from_secs(metrics::SAMPLE_TIMEOUT_SECS);
/// 首轮连接与退避重连的错开上限（按服务器 id 取值）。
const FIRST_CONNECT_SPREAD_SECS: u64 = 4;
const BACKOFF_SPREAD_SECS: u64 = 3;
/// 远端错误文本的截断长度：这一句要进内存状态、再进界面悬停，长度不该由对端定。
const ERROR_CLIP_CHARS: usize = 160;

/// 一台主机的常驻状态：会话、隧道规则、最近一次资源采集。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerHostStatus {
    pub server_id: String,
    pub server_name: String,
    /// SSH 会话是否在线。
    pub connected: bool,
    /// 连续重连次数，连上即清零。
    pub retries: u32,
    /// 最近一次失败原因（连接失败 / 会话断开 / 单条转发失败）。
    pub last_error: Option<String>,
    pub connected_at: Option<String>,
    /// 本次在线期间转发过的连接数。
    pub forwarded: u64,
    pub rules: Vec<TunnelRuleStatus>,
    /// 最近一次成功采集的占用；没有就是「此刻没有数字」，界面不许拿旧的顶上。
    pub metrics: Option<HostMetrics>,
    /// 采不到的原因，与 [`ServerHostStatus::metrics`] 成对更新。
    pub metrics_error: Option<String>,
    /// 这一组数字是哪一刻采的，让界面能说清它是刚采的还是断线前留的。
    pub sampled_at: Option<String>,
}

/// 一个常驻任务的指纹：连接参数 + 启用规则。
///
/// 必须把主机与凭据一起算：只比规则的话，用户改了密码或换了主机之后，老任务还会拿
/// 旧参数一直重连，界面上却显示「重连中」，看不出是配置已经变了。
/// 只留哈希，避免明文口令在内存里多存一份。
pub fn task_fingerprint(server: &ServerConfig) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    server.host.hash(&mut hasher);
    server.port.hash(&mut hasher);
    server.username.hash(&mut hasher);
    serde_json::to_string(&server.auth)
        .unwrap_or_default()
        .hash(&mut hasher);
    fingerprint(&server.tunnels).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 按服务器 id 取一个稳定的小整数秒数，用来错开首轮连接与退避重连。
///
/// 用 id 而不是随机数：同一台的错开量在重建任务后依旧不变，测试也钉得住。
fn spread_secs(seed: &str, span: u64) -> u64 {
    if span == 0 {
        return 0;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut hasher);
    hasher.finish() % span
}

/// 截断来自远端的文本，保证按字符边界切（远端输出是 UTF-8，按字节切会劈开汉字）。
///
/// 传输那一路的失败原因同样来自远端（SFTP 的错误串长度由服务器决定），共用这一份口径。
pub(crate) fn clip(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let clipped: String = trimmed.chars().take(ERROR_CLIP_CHARS).collect();
    if clipped.chars().count() == trimmed.chars().count() {
        return Some(clipped);
    }
    Some(format!("{clipped}…"))
}

// ---------------------------------------------------------------------------
// 常驻管理器
// ---------------------------------------------------------------------------

/// 已绑定的一条规则：监听句柄跨重连复用，端口不会在退避间隙被别人抢走。
struct BoundRule {
    rule: TunnelRule,
    listener: Arc<TcpListener>,
}

/// 一台常驻任务当前活着的那条会话，供上层（终端）在同一会话上开通道。
///
/// 只在这里发布「这一轮」的会话：轮次在同一个 `supervise` 任务里是串行的，所以槽里
/// 最多只有一条会话；任务被 abort 时槽随条目一起消失，不会留下一条没人管的旧句柄。
type LiveSession = Arc<Mutex<Option<Arc<SshClient>>>>;

struct RunningLink {
    abort: tokio::task::AbortHandle,
    fingerprint: String,
    status: Arc<Mutex<ServerHostStatus>>,
    session: LiveSession,
}

/// 常驻连接登记表：每台已登记的服务器一个任务。
#[derive(Default)]
pub struct HostLinkManager {
    running: Mutex<HashMap<String, RunningLink>>,
}

impl HostLinkManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 按当前配置对齐：缺的补上、连接参数或规则变了的重建、服务器已删的下线。返回在跑的台数。
    ///
    /// 必须在 tokio 运行时上下文里调用（GUI 的命令与启动任务都满足）。
    pub fn sync(&self, servers: &[ServerConfig]) -> usize {
        let mut running = lock(&self.running);
        let desired: HashMap<String, ServerConfig> = servers
            .iter()
            .map(|server| (server.id.clone(), server.clone()))
            .collect();

        let stale: Vec<String> = running
            .iter()
            .filter(|(id, entry)| match desired.get(*id) {
                Some(server) => task_fingerprint(server) != entry.fingerprint,
                None => true,
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(entry) = running.remove(&id) {
                entry.abort.abort();
            }
        }

        for (id, server) in desired {
            if running.contains_key(&id) {
                continue;
            }
            let mark = task_fingerprint(&server);
            let status = initial_status(&server);
            let session: LiveSession = Arc::new(Mutex::new(None));
            let handle = tokio::spawn(supervise(server, status.clone(), session.clone()));
            running.insert(
                id,
                RunningLink {
                    abort: handle.abort_handle(),
                    fingerprint: mark,
                    status,
                    session,
                },
            );
        }
        running.len()
    }

    /// 强制重建这一台的会话（配置没变、但用户点了「重新连接」时用）。
    ///
    /// 重建后的任务一连上就采一次，所以这一个调用同时也是「立刻重采」。
    pub fn restart(&self, server: &ServerConfig) {
        let mut running = lock(&self.running);
        if let Some(entry) = running.remove(&server.id) {
            entry.abort.abort();
        }
        let status = initial_status(server);
        let session: LiveSession = Arc::new(Mutex::new(None));
        let handle = tokio::spawn(supervise(server.clone(), status.clone(), session.clone()));
        running.insert(
            server.id.clone(),
            RunningLink {
                abort: handle.abort_handle(),
                fingerprint: task_fingerprint(server),
                status,
                session,
            },
        );
    }

    /// 当前所有在跑（或刚失败退出）的会话状态，按服务器名排序。
    pub fn status(&self) -> Vec<ServerHostStatus> {
        let running = lock(&self.running);
        let mut list: Vec<ServerHostStatus> = running
            .values()
            .map(|entry| lock(&entry.status).clone())
            .collect();
        list.sort_by(|a, b| a.server_name.cmp(&b.server_name));
        list
    }

    /// 这一台当前在线的那条会话：终端要在它上面再开一条 PTY 通道，而不是自己再认证一次。
    ///
    /// 会话已判死（`is_closed`）时返回 None，而不是把那条连不上的句柄交出去——拿着它
    /// 只会开出一个永远等不到数据的通道，界面再显示成「终端已打开」。
    pub fn client(&self, server_id: &str) -> Option<Arc<SshClient>> {
        let running = lock(&self.running);
        let entry = running.get(server_id)?;
        let client = lock(&entry.session).clone()?;
        if client.is_closed() {
            return None;
        }
        Some(client)
    }

    /// 退出时收掉所有会话与监听：本机端口立刻释放，远端也不会留下悬着的通道。
    pub fn stop_all(&self) {
        let mut running = lock(&self.running);
        for (_, entry) in running.drain() {
            entry.abort.abort();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 一台常驻任务的初始状态：未连接、无数字，规则按启用项列出。
fn initial_status(server: &ServerConfig) -> Arc<Mutex<ServerHostStatus>> {
    Arc::new(Mutex::new(ServerHostStatus {
        server_id: server.id.clone(),
        server_name: server.name.clone(),
        connected: false,
        retries: 0,
        last_error: None,
        connected_at: None,
        forwarded: 0,
        metrics: None,
        // 一次都没采过：这不是「采不到」，界面该说「采集中」而不是给一个红色的失败。
        metrics_error: None,
        sampled_at: None,
        rules: server
            .active_tunnels()
            .into_iter()
            .map(|rule| TunnelRuleStatus {
                rule_id: rule.id.clone(),
                local_port: rule.local_port,
                remote_label: rule.remote_label(),
                bound: false,
                active: false,
                error: None,
            })
            .collect(),
    }))
}

/// 一台服务器的主循环：补绑本机端口 -> 建会话 -> 转发 + 探针直到会话不可用 -> 退避重来。
async fn supervise(
    server: ServerConfig,
    status: Arc<Mutex<ServerHostStatus>>,
    session: LiveSession,
) {
    let rules = server.active_tunnels();
    let mut bound: Vec<BoundRule> = Vec::with_capacity(rules.len());
    let mut backoff = RECONNECT_MIN_SECS;

    // 开机 / 唤醒时 N 台一起起来，错开首轮，别在同一分钟里堆一串认证。
    tokio::time::sleep(Duration::from_secs(spread_secs(
        &server.id,
        FIRST_CONNECT_SPREAD_SECS,
    )))
    .await;

    loop {
        // 每一轮都补齐还没绑上的本机端口：启动那会儿端口被别的应用占着、或刚点过「重新连接」
        // 而旧监听还没来得及释放，都不该让这条会话从此死掉。
        bind_pending_rules(&rules, &mut bound, &status).await;

        let started_at = Instant::now();
        let outcome = connect_and_serve(&server, &bound, &status, &session).await;
        let held = started_at.elapsed();
        match outcome {
            // 会话正常结束（多为网络中断或 sshd 重启）：单条转发失败留下的错误不覆盖。
            Ok(()) => with_status(&status, |current| {
                current.connected = false;
                current.forwarded = 0;
                for item in current.rules.iter_mut() {
                    item.active = false;
                }
                mark_stale(current, "连接已断开，正在重连");
                if current.last_error.is_none() {
                    current.last_error = clip("连接已断开，正在重连");
                }
            }),
            Err(err) => with_status(&status, move |current| {
                current.connected = false;
                current.forwarded = 0;
                current.retries += 1;
                current.last_error = clip(&err.to_string());
                for item in current.rules.iter_mut() {
                    item.active = false;
                }
                mark_stale(current, &err.to_string());
            }),
        }

        // 撑过门槛时长说明不是「一连就断」，退避重新从最小值起。
        if held >= Duration::from_secs(STABLE_SESSION_SECS) {
            backoff = RECONNECT_MIN_SECS;
        }
        let wait = Duration::from_secs(backoff)
            + Duration::from_secs(spread_secs(&server.id, BACKOFF_SPREAD_SECS));
        tokio::time::sleep(wait).await;
        backoff = (backoff * 2).min(RECONNECT_MAX_SECS);
    }
}

/// 会话一断，这一台的数字当场作废：留着上一个成功值，界面上就是一个看着正常其实
/// 是几分钟前的数字——那是比缺一行更糟的展示。
fn mark_stale(current: &mut ServerHostStatus, reason: &str) {
    current.metrics = None;
    current.sampled_at = None;
    current.metrics_error = clip(reason);
}

/// 把还没绑上的本机端口补上并写进状态；已绑的沿用，退避间隙不会被别的应用抢走。
async fn bind_pending_rules(
    rules: &[TunnelRule],
    bound: &mut Vec<BoundRule>,
    status: &Arc<Mutex<ServerHostStatus>>,
) {
    for rule in rules {
        if bound
            .iter()
            .any(|item| item.rule.local_port == rule.local_port)
        {
            continue;
        }
        let port = rule.local_port;
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => {
                bound.push(BoundRule {
                    rule: rule.clone(),
                    listener: Arc::new(listener),
                });
                mark_rule(status, port, true, None);
            }
            Err(err) => mark_rule(
                status,
                port,
                false,
                clip(&format!("本机端口 {port} 绑定失败: {err}")),
            ),
        }
    }
}

/// 单条规则的监听结果。`active` 只有在会话已在线时才允许点亮，避免没连上也显示「可转发」。
fn mark_rule(
    status: &Arc<Mutex<ServerHostStatus>>,
    local_port: u16,
    ok: bool,
    error: Option<String>,
) {
    with_status(status, move |current| {
        if let Some(item) = current
            .rules
            .iter_mut()
            .find(|item| item.local_port == local_port)
        {
            item.bound = ok;
            item.active = ok && current.connected;
            item.error = error;
        }
    });
}

/// 建立一次会话，在上面同时跑隧道转发与资源探针，直到会话不可用。
async fn connect_and_serve(
    server: &ServerConfig,
    bound: &[BoundRule],
    status: &Arc<Mutex<ServerHostStatus>>,
    session: &LiveSession,
) -> Result<()> {
    let client = Arc::new(SshClient::connect_tunnel(server, CONNECT_TIMEOUT_SECS).await?);

    with_status(status, |current| {
        current.connected = true;
        current.retries = 0;
        current.last_error = None;
        current.forwarded = 0;
        current.connected_at = Some(now_string());
        for item in current.rules.iter_mut() {
            item.active = item.bound;
            if item.bound {
                item.error = None;
            }
        }
    });

    // 会话一在线就发布出去：终端要的是同一条会话，不是再一次认证。
    *lock(session) = Some(client.clone());

    let (stop_tx, stop_rx) = watch::channel(false);
    let mut tasks = JoinSet::new();
    for item in bound {
        tasks.spawn(accept_loop(
            client.clone(),
            item.rule.clone(),
            item.listener.clone(),
            stop_rx.clone(),
            status.clone(),
        ));
    }
    tasks.spawn(watch_liveness(
        client.clone(),
        stop_tx.clone(),
        stop_rx.clone(),
    ));
    tasks.spawn(probe_loop(client.clone(), stop_rx.clone(), status.clone()));

    // 任一侧退出就代表这一轮转发作废：广播停止、等在途任务收口。
    let first = tasks.join_next().await;
    let _ = stop_tx.send(true);
    if tokio::time::timeout(
        Duration::from_secs(DRAIN_TIMEOUT_SECS),
        async { while tasks.join_next().await.is_some() {} },
    )
    .await
    .is_err()
    {
        tasks.abort_all();
    }

    let reason = match first {
        Some(Ok(Ok(()))) => Ok(()),
        Some(Ok(Err(err))) => Err(err),
        // 任务被取消（退出清理）或 panic：当作会话不可用，交给外层退避重连。
        _ => Err(CoreError::ssh("连接已断开，正在重连")),
    };
    // 这一轮作废，槽里的句柄同步收回：留着它，终端会拿到一条看着在线、实际再也收不到
    // 任何数据的会话——而重新连上的那一条已经在下一轮里了。
    *lock(session) = None;
    // client 在此处随作用域结束而释放：会话关闭，本机端口仍由 supervise 持有。
    reason
}

/// 接受本机连接并转成一条 direct-tcpip 通道；收到停止信号就退出。
async fn accept_loop(
    client: Arc<SshClient>,
    rule: TunnelRule,
    listener: Arc<TcpListener>,
    mut stop: watch::Receiver<bool>,
    status: Arc<Mutex<ServerHostStatus>>,
) -> Result<()> {
    loop {
        if *stop.borrow_and_update() {
            return Ok(());
        }
        let accepted = tokio::select! {
            result = listener.accept() => result,
            _ = stop.changed() => return Ok(()),
        };
        let (stream, _) = match accepted {
            Ok(pair) => pair,
            // 一次 accept 失败不许把整条会话拆了重来：会话是全部连接共用的那一条 SSH 连接，
            // 重来会掐掉其它正在转发的连接与正在跑的探针，而这种失败多半只关系到刚那一个连接
            // （本机那端在握手完成前就被重置、进程句柄一时不够）。端口真被抢走的场合
            // 也只需跳过这一轮，绑定由 `supervise` 的下一轮补齐。
            Err(err) => {
                let message = clip(&format!(
                    "本机端口 {} 接受连接失败: {err}",
                    rule.local_port
                ));
                with_status(&status, move |current| {
                    current.last_error = message;
                });
                // 歇一小会儿再回去 accept：持续失败时不至于空转把 CPU 吃满。
                tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                continue;
            }
        };

        {
            let mut guard = lock(&status);
            guard.forwarded += 1;
        }

        let client = client.clone();
        let host = rule.remote_host.clone();
        let remote_port = rule.remote_port;
        let counter = status.clone();
        let mut conn_stop = stop.clone();
        // 每条连接独立一个任务：一条转发结束（或对端失败）不应影响别的连接。
        tokio::spawn(async move {
            // 但任务必须跟着这一轮一起收：攥着 `Arc<SshClient>` 不放的那条空连接
            // （浏览器 keepalive 最典型）会让这轮作废的会话永远关不掉，
            // 点「重新连接」之后旧会话还挂在上面。
            let outcome = tokio::select! {
                result = client.forward_tcp(stream, &host, remote_port) => result,
                _ = conn_stop.changed() => return,
            };
            if let Err(err) = outcome {
                // 单条连接失败只记账到这台的最近错误：会话还在，用户下次请求就能恢复。
                let message = clip(&err.to_string());
                let mut guard = lock(&counter);
                guard.last_error = message;
            }
        });
    }
}

/// 会话存活探测：russh 句柄关闭即广播停止，让主循环重连。
async fn watch_liveness(
    client: Arc<SshClient>,
    stop: watch::Sender<bool>,
    mut rx: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        if client.is_closed() {
            let _ = stop.send(true);
            return Err(CoreError::ssh("连接已断开，正在重连"));
        }
        if tokio::time::timeout(Duration::from_secs(HEALTH_CHECK_SECS), rx.changed())
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
}

/// 资源探针：在既有会话上每隔一个间隔采一次，结果只进内存状态，不落盘也不推事件。
///
/// 一次采集失败不等于会话失败——通道开不出来、脚本没跑完都可能是远端那侧的问题，
/// 所以这里只把这一台的数字置空并记下原因，重连由 `watch_liveness` 判。
///
/// **超时是例外**，它要让这一轮整个作废：预算到点等于把 `exec` 通道在半空中掐掉，
/// 而 russh 对被 drop 的普通通道不补发 CHANNEL_CLOSE（只有 `into_stream()` 那条路会），
/// 于是远端那个还活着的进程在服务器那边继续占一个 session 槽——槽位是和隧道转发
/// 一起算的（OpenSSH `MaxSessions`，默认 10）。慢机器上每 20 秒掐一次，几轮就能把这一台的
/// 槽占满，之后隧道和探针一起打不开，而会话本身看着完全健康，`watch_liveness` 不会去重连。
/// 换掉整条连接是唯一能把这些槽确定收回来的动作，代价只是一次握手。
async fn probe_loop(
    client: Arc<SshClient>,
    mut stop: watch::Receiver<bool>,
    status: Arc<Mutex<ServerHostStatus>>,
) -> Result<()> {
    loop {
        if *stop.borrow_and_update() {
            return Ok(());
        }
        // 一连上就采一次：界面不用等满一个间隔才看到第一个数字。
        match tokio::time::timeout(SAMPLE_DEADLINE, metrics::sample(&client)).await {
            Ok(Ok(reading)) => with_status(&status, move |current| {
                current.metrics = Some(reading);
                current.metrics_error = None;
                current.sampled_at = Some(now_string());
            }),
            Ok(Err(err)) => with_status(&status, move |current| {
                mark_stale(current, &err.to_string());
            }),
            Err(_elapsed) => {
                return Err(CoreError::ssh(
                    "资源采集超时，正在重建连接（避免通道占住服务器的会话额度）",
                ))
            }
        }
        if client.is_closed() {
            return Err(CoreError::ssh("连接已断开，正在重连"));
        }
        // 停止信号优先：这一轮作废时探针不许攥着 Arc<SshClient> 不放，
        // 否则旧会话永远关不掉，新一轮的会话只是多出来的一条。
        if tokio::time::timeout(PROBE_INTERVAL, stop.changed())
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
}

fn with_status<F>(status: &Arc<Mutex<ServerHostStatus>>, f: F)
where
    F: FnOnce(&mut ServerHostStatus),
{
    let mut guard = lock(status);
    f(&mut guard);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SshAuth;

    fn rule(local: u16, host: &str, remote: u16) -> TunnelRule {
        TunnelRule {
            id: format!("r{local}"),
            local_port: local,
            remote_host: host.to_string(),
            remote_port: remote,
            enabled: true,
        }
    }

    fn server(id: &str, tunnels: Vec<TunnelRule>) -> ServerConfig {
        ServerConfig {
            id: id.to_string(),
            name: id.to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth: SshAuth::Password {
                password: "x".to_string(),
            },
            default_target_dir: String::new(),
            db_backup: None,
            backup_target_id: None,
            supabase_url: None,
            tunnels,
            created_at: String::new(),
        }
    }

    async fn free_port() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("找一个空闲端口");
        let port = listener.local_addr().expect("端口号").port();
        drop(listener);
        port
    }

    async fn wait_for<F>(manager: &HostLinkManager, mut pred: F) -> Option<ServerHostStatus>
    where
        F: FnMut(&ServerHostStatus) -> bool,
    {
        // 上限约 20s：首轮错开最多 4 秒，端口空出来之后的自愈要等下一轮退避。
        for _ in 0..800 {
            for status in manager.status() {
                if pred(&status) {
                    return Some(status);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        None
    }

    /// 服务器连不上时也要把本机端口占住，并且停止后立刻释放。
    ///
    /// 「打开软件就自动接上」可靠的就是这一半；真正转发包与采集要一台活服务器，归手工验证。
    #[tokio::test]
    async fn unreachable_server_still_holds_and_releases_the_local_port() {
        let local = free_port().await;
        let dead = free_port().await;
        let mut target = server("a", vec![rule(local, "127.0.0.1", dead)]);
        target.host = "127.0.0.1".to_string();
        target.port = dead;

        let manager = HostLinkManager::new();
        assert_eq!(manager.sync(&[target.clone()]), 1);

        let bound = wait_for(&manager, |status| {
            status.rules.iter().any(|item| item.bound)
        })
        .await;
        assert!(bound.is_some(), "本机端口应当绑定成功");
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", local))
                .await
                .is_ok(),
            "监听应当真的连得上"
        );

        let failed = wait_for(&manager, |status| status.last_error.is_some()).await;
        let failed = failed.expect("连不上的服务器要把原因记下来");
        assert!(!failed.connected, "没连上就不许显示已连接");
        assert!(failed.metrics.is_none(), "没连上就没有数字");

        manager.stop_all();
        let mut released = false;
        for _ in 0..120 {
            if TcpListener::bind(("127.0.0.1", local)).await.is_ok() {
                released = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(released, "停止后本机端口必须交还出来");
    }

    /// 没配隧道的服务器也要起任务：探针要的是每一台，不只是转发了端口的那些。
    #[tokio::test]
    async fn server_without_tunnel_rules_still_gets_a_task() {
        let dead = free_port().await;
        let mut target = server("a", vec![]);
        target.host = "127.0.0.1".to_string();
        target.port = dead;

        let manager = HostLinkManager::new();
        assert_eq!(manager.sync(&[target]), 1);
        let status = wait_for(&manager, |status| status.rules.is_empty() && status.retries > 0)
            .await
            .expect("没有规则也要建会话并记下失败");
        assert!(!status.connected);
        assert!(status.metrics.is_none());
        manager.stop_all();
    }

    /// 没连上的那一条会话不许出现在槽里：终端拿到句柄就会开出一条永远等不到数据的通道，
    /// 而界面已经把它当成「终端已打开」。
    #[tokio::test]
    async fn no_live_session_is_published_until_the_server_connects() {
        let dead = free_port().await;
        let mut target = server("a", vec![]);
        target.host = "127.0.0.1".to_string();
        target.port = dead;

        let manager = HostLinkManager::new();
        assert_eq!(manager.sync(&[target]), 1);
        let failed = wait_for(&manager, |status| status.retries > 0)
            .await
            .expect("连不上要记下重试");
        assert!(!failed.connected);
        assert!(manager.client("a").is_none(), "没连上就不该有可复用的会话");
        assert!(manager.client("不存在的 id").is_none());

        manager.stop_all();
        assert!(manager.client("a").is_none(), "停止之后条目连带它的槽一起消失");
    }

    /// 绑定结果与错误要成对更新。
    ///
    /// 端口空出来之后旧的「绑定失败」必须清掉，否则界面会一直红着；反过来没连上服务器时
    /// 即便端口绑上了也不许报「可转发」。
    #[test]
    fn mark_rule_clears_error_when_bound() {
        let status = Arc::new(Mutex::new(ServerHostStatus {
            server_id: "a".to_string(),
            server_name: "a".to_string(),
            connected: false,
            retries: 0,
            last_error: None,
            connected_at: None,
            forwarded: 0,
            metrics: None,
            metrics_error: Some("本机端口 18080 绑定失败: 占用".to_string()),
            sampled_at: None,
            rules: vec![TunnelRuleStatus {
                rule_id: "r".to_string(),
                local_port: 18080,
                remote_label: "127.0.0.1:8080".to_string(),
                bound: false,
                active: false,
                error: Some("本机端口 18080 绑定失败: 占用".to_string()),
            }],
        }));

        mark_rule(&status, 18080, true, None);
        {
            let guard = lock(&status);
            let item = &guard.rules[0];
            assert!(item.bound, "端口绑上");
            assert!(!item.active, "没连上就不许说可转发");
            assert!(item.error.is_none(), "绑定成功后旧的错误要消失");
        }

        // 反过来：没连上时 active 必须为 false，即便 bound 为真。
        // guard 必须先放掉——这把锁是非重入的，抱着它再调 mark_rule 会自锁死。
        mark_rule(&status, 18080, false, Some("端口被抢".to_string()));
        let item = lock(&status).rules[0].clone();
        assert!(!item.bound && !item.active && item.error.is_some());
    }

    /// 本机端口一时被别的应用占住，不能让这条会话从此死掉。
    ///
    /// 开机顺序不可控（软件可能比服务器上那个端口先起，用户也可能连点两次「重新连接」），
    /// 占用的那一方退出后要自己把监听补回来。
    #[tokio::test]
    async fn busy_local_port_is_retried_until_it_frees_up() {
        let local = free_port().await;
        let dead = free_port().await;
        let mut target = server("a", vec![rule(local, "127.0.0.1", dead)]);
        target.host = "127.0.0.1".to_string();
        target.port = dead;

        let blocker = TcpListener::bind(("127.0.0.1", local)).await.expect("先占住端口");
        let manager = HostLinkManager::new();
        assert_eq!(manager.sync(&[target]), 1);

        tokio::time::sleep(Duration::from_millis(400)).await;
        let blocked = manager.status().into_iter().next().expect("已登记");
        assert!(
            blocked.rules.iter().all(|item| !item.bound),
            "端口被占时不许谎报已绑定"
        );

        drop(blocker);
        let healed = wait_for(&manager, |status| status.rules.iter().any(|item| item.bound)).await;
        assert!(healed.is_some(), "端口空出来之后要自己绑上");
        manager.stop_all();
    }

    /// 断线时数字必须当场作废，不能留着一个看着正常其实是几分钟前采的数。
    #[test]
    fn disconnect_invalidates_the_last_reading() {
        let status = Arc::new(Mutex::new(ServerHostStatus {
            server_id: "a".to_string(),
            server_name: "a".to_string(),
            connected: true,
            retries: 0,
            last_error: None,
            connected_at: Some("2026-11-05 03:00:00".to_string()),
            forwarded: 0,
            metrics: Some(HostMetrics {
                cpu_percent: 42,
                cpu_cores: 4,
                memory_total_bytes: 1024,
                memory_used_bytes: 512,
            }),
            metrics_error: None,
            sampled_at: Some("2026-11-05 03:00:00".to_string()),
            rules: vec![],
        }));

        with_status(&status, move |current| mark_stale(current, "连接已断开，正在重连"));
        let guard = lock(&status);
        assert!(guard.metrics.is_none(), "断开后不许留着上一个数字");
        assert!(guard.sampled_at.is_none(), "作废的读数不该还有一个采集时刻");
        assert_eq!(
            guard.metrics_error.as_deref(),
            Some("连接已断开，正在重连"),
            "但要有一个原因"
        );
    }

    /// Rust 与 TS 是手工镜像的，字段名对不上编译器不会报错、运行时才炸。
    /// 这条测试把界面读到的键名钉死。
    #[test]
    fn status_json_keys_match_the_frontend_types() {
        let status = lock(&initial_status(&server(
            "a",
            vec![rule(18080, "127.0.0.1", 8080)],
        )))
        .clone();
        let value = serde_json::to_value(&status).expect("序列化");
        for key in [
            "serverId",
            "serverName",
            "connected",
            "retries",
            "lastError",
            "connectedAt",
            "forwarded",
            "rules",
            "metrics",
            "metricsError",
            "sampledAt",
        ] {
            assert!(value.get(key).is_some(), "缺字段 {key}");
        }
        let item = &value["rules"][0];
        for key in ["ruleId", "localPort", "remoteLabel", "bound", "active", "error"] {
            assert!(item.get(key).is_some(), "缺字段 {key}");
        }
    }

    /// 一次都没采过不是「采不到」：初始状态只许留空，让界面说「采集中」而不是给一个红色失败。
    #[test]
    fn fresh_status_has_no_failure_to_report() {
        let status = lock(&initial_status(&server("a", vec![]))).clone();
        assert!(status.metrics.is_none());
        assert!(
            status.metrics_error.is_none(),
            "没采过就不许先写上一个失败原因"
        );
        assert!(status.sampled_at.is_none());
    }

    #[test]
    fn metrics_json_keys_match_the_frontend_types() {
        let value = serde_json::to_value(HostMetrics {
            cpu_percent: 12,
            cpu_cores: 4,
            memory_total_bytes: 1024,
            memory_used_bytes: 512,
        })
        .expect("序列化");
        for key in [
            "cpuPercent",
            "cpuCores",
            "memoryTotalBytes",
            "memoryUsedBytes",
        ] {
            assert!(value.get(key).is_some(), "缺字段 {key}");
        }
    }

    /// 远端回给我们的文本长度不由我们定（会话没有校验主机密钥，中间人想写多少就写多少），
    /// 而这一句要一路进界面悬停：按字符截断，且不许按字节劈开汉字。
    #[test]
    fn remote_error_text_is_clipped_on_char_bounds() {
        let long = "错".repeat(ERROR_CLIP_CHARS * 3);
        let clipped = clip(&long).expect("非空");
        assert_eq!(clipped.chars().count(), ERROR_CLIP_CHARS + 1, "多出的是省略号");
        assert!(clipped.ends_with('…'));

        assert_eq!(clip("  已经够了  ").as_deref(), Some("已经够了"));
        assert_eq!(clip("   "), None, "空文本不值得占一个错误位");
    }

    /// 采集预算要落在常驻循环里排得开：间隔必须盖过采样窗口，超时不能把间隔顶成
    /// 长期排队（慢机器最多顺延几轮，之后下一轮照常）。
    #[test]
    fn sampling_budget_stays_inside_the_probe_interval() {
        assert!(
            PROBE_INTERVAL > Duration::from_secs(metrics::SAMPLE_WINDOW_SECS),
            "间隔不大于采样窗口时通道会一直排在上一轮"
        );
        assert!(metrics::SAMPLE_TIMEOUT_SECS as u64 > metrics::SAMPLE_WINDOW_SECS);
        assert!(
            metrics::SAMPLE_TIMEOUT_SECS as u64 <= PROBE_INTERVAL.as_secs() * 4,
            "一次采集的超时不该把整轮的节奏拖到看不出来是在跑"
        );
        // 掐表的必须是这一层，且必须早于 exec 内部的兜底上限：换会话这条保障只有
        // 「外层先响」才走得到，兜底上限先响就变成记一个错误、继续用同一条会话。
        assert_eq!(SAMPLE_DEADLINE, Duration::from_secs(metrics::SAMPLE_TIMEOUT_SECS));
        assert!(SAMPLE_DEADLINE < Duration::from_secs(metrics::SAMPLE_CEILING_SECS));
    }

    /// 错开量按 id 稳定：同一台重建任务后还是同一个偏移，测试与真机行为才对得上。
    #[test]
    fn spread_is_stable_and_within_span() {
        assert_eq!(spread_secs("s1", 4), spread_secs("s1", 4));
        for id in ["a", "b", "c", "深圳-1", "10.0.0.9"] {
            assert!(spread_secs(id, 4) < 4);
            assert!(spread_secs(id, 3) < 3);
        }
        assert_eq!(spread_secs("a", 0), 0, "span 为 0 就是不偏移");
    }

    /// 改了口令或主机必须重算指纹，否则老任务会拿旧凭据一直重连。
    #[test]
    fn task_fingerprint_follows_connection_and_enabled_rules() {
        let base = server("a", vec![rule(18080, "127.0.0.1", 8080)]);
        assert_eq!(task_fingerprint(&base), task_fingerprint(&base.clone()));

        // 改名不影响连接，不该把在跑的会话踢掉重连。
        let mut renamed = base.clone();
        renamed.name = "改了个名字".to_string();
        renamed.default_target_dir = "/srv/other".to_string();
        assert_eq!(task_fingerprint(&base), task_fingerprint(&renamed));

        let mut other_password = base.clone();
        other_password.auth = SshAuth::Password {
            password: "新的".to_string(),
        };
        assert_ne!(task_fingerprint(&base), task_fingerprint(&other_password));

        let mut other_host = base.clone();
        other_host.host = "10.0.0.2".to_string();
        assert_ne!(task_fingerprint(&base), task_fingerprint(&other_host));

        // 禁用的规则不参与指纹：勾掉再勾回来不该断流。
        let mut disabled = base.clone();
        disabled.tunnels.push({
            let mut extra = rule(19000, "127.0.0.1", 5432);
            extra.enabled = false;
            extra
        });
        assert_eq!(task_fingerprint(&base), task_fingerprint(&disabled));

        let mut enabled = base.clone();
        enabled.tunnels.push(rule(19000, "127.0.0.1", 5432));
        assert_ne!(task_fingerprint(&base), task_fingerprint(&enabled));
    }
}
