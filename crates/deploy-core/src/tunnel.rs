//! SSH 隧道：把本机端口转发到该服务器上的任意地址，并在应用运行期间保活重连。
//!
//! 规则存在 `ServerConfig.tunnels` 里（不落单独的盘），有启用项的服务器在应用启动时
//! 就被 [`TunnelManager::sync`] 拉起一个常驻任务：绑定本机端口 -> 建立 SSH 连接 ->
//! 每条本机连接开一个 direct-tcpip 通道。会话断了只重连、不退出，所以笔记本合盖唤醒、
//! 服务器重启之后转发会自己回来。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::error::{CoreError, Result};
use crate::models::{now_string, ServerConfig, TunnelRule};
use crate::ssh::SshClient;

/// 建立 SSH 连接的超时（含认证）。隧道是常驻任务，这里不必太长。
const CONNECT_TIMEOUT_SECS: u64 = 15;
/// 重连退避的起止与「会话算稳定」的门槛：秒断的服务器不会把间隔顶到最大。
const RECONNECT_MIN_SECS: u64 = 3;
const RECONNECT_MAX_SECS: u64 = 60;
const STABLE_SESSION_SECS: u64 = 60;
/// 会话存活探测间隔：russh 的句柄在会话任务结束后 `is_closed()` 即为真。
const HEALTH_CHECK_SECS: u64 = 2;
/// 退出前等在途转发任务收口的最长时间，超时直接 abort。
const DRAIN_TIMEOUT_SECS: u64 = 2;

/// 一条规则的监听状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelRuleStatus {
    pub rule_id: String,
    pub local_port: u16,
    /// 展示用：`127.0.0.1:8080`。
    pub remote_label: String,
    /// 本机端口是否已绑定成功（绑定失败通常是端口被占用）。
    pub bound: bool,
    /// 隧道当前是否可用来转这一条（绑定成功 + SSH 在线）。
    pub active: bool,
    pub error: Option<String>,
}

/// 一台服务器的隧道状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerTunnelStatus {
    pub server_id: String,
    pub server_name: String,
    /// SSH 会话是否在线。
    pub connected: bool,
    /// 连续重连次数，连上即清零。
    pub retries: u32,
    /// 最近一次失败原因（连接失败 / 会话断开）。
    pub last_error: Option<String>,
    pub connected_at: Option<String>,
    /// 本次在线期间转发过的连接数。
    pub forwarded: u64,
    pub rules: Vec<TunnelRuleStatus>,
}

/// 规则集合的指纹：本机端口与远端地址一致就无需重建监听。
pub fn fingerprint(rules: &[TunnelRule]) -> String {
    rules
        .iter()
        .filter(|rule| rule.enabled)
        .map(|rule| format!("{}>{}:{}", rule.local_port, rule.remote_host, rule.remote_port))
        .collect::<Vec<String>>()
        .join(";")
}

/// 一个隧道任务的指纹：连接参数 + 启用规则。
///
/// 必须把主机与凭据一起算：只比规则的话，用户改了密码或换了主机之后，老任务还会拿
/// 旧参数一直重连，界面上却显示「重连中」，看不出是配置已经变了。
/// 只留哈希，避免明文口令在内存里多存一份。
pub fn task_fingerprint(server: &ServerConfig) -> String {
    use std::hash::{Hash, Hasher};
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

/// 规范化并校验一组规则：端口合法、远端地址可用、同一台内本机端口不重复，id 为空时补齐。
pub fn normalize_rules(rules: &[TunnelRule]) -> Result<Vec<TunnelRule>> {
    let mut normalized: Vec<TunnelRule> = Vec::with_capacity(rules.len());
    for rule in rules {
        if rule.local_port == 0 || rule.remote_port == 0 {
            return Err(CoreError::config("隧道端口必须在 1-65535 之间"));
        }
        let host = rule.remote_host.trim();
        if host.is_empty() {
            return Err(CoreError::config(format!(
                "端口 {} 的转发目标地址不能为空",
                rule.local_port
            )));
        }
        if !is_valid_host(host) {
            return Err(CoreError::config(format!(
                "转发目标地址不可用：{host}（应为 IP 或主机名，端口写在端口栏）"
            )));
        }
        if normalized
            .iter()
            .any(|item| item.enabled && rule.enabled && item.local_port == rule.local_port)
        {
            return Err(CoreError::config(format!(
                "本机端口重复: {}（同一台服务器下不能有两条启用的规则占同一个端口）",
                rule.local_port
            )));
        }
        normalized.push(TunnelRule {
            id: if rule.id.is_empty() {
                crate::models::new_id()
            } else {
                rule.id.clone()
            },
            local_port: rule.local_port,
            remote_host: host.to_string(),
            remote_port: rule.remote_port,
            enabled: rule.enabled,
        });
    }
    Ok(normalized)
}

/// direct-tcpip 只接受「地址 + 端口」两段，带协议前缀、带端口、含空白的写法拼进去
/// 只会得到一个说不清的失败，所以在录入处就拒掉。
fn is_valid_host(host: &str) -> bool {
    if host.contains(char::is_whitespace) || host.contains('/') {
        return false;
    }
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    !host.contains(':')
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// 跨服务器检查本机端口冲突：本机只有一套端口空间，两台服务器抢同一个端口时后绑的一定失败。
pub fn assert_local_ports_free(
    servers: &[ServerConfig],
    editing_server_id: &str,
    rules: &[TunnelRule],
) -> Result<()> {
    let taken: Vec<(u16, String)> = servers
        .iter()
        .filter(|server| server.id != editing_server_id)
        .flat_map(|server| {
            server
                .active_tunnels()
                .into_iter()
                .map(move |rule| (rule.local_port, server.name.clone()))
        })
        .collect();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        if let Some((_, name)) = taken.iter().find(|(port, _)| *port == rule.local_port) {
            return Err(CoreError::config(format!(
                "本机端口 {} 已被服务器「{name}」的隧道占用，请换一个端口",
                rule.local_port
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 常驻管理器
// ---------------------------------------------------------------------------

/// 已绑定的一条规则：监听句柄跨重连复用，端口不会在退避间隙被别人抢走。
struct BoundRule {
    rule: TunnelRule,
    listener: Arc<TcpListener>,
}

struct RunningTunnel {
    abort: tokio::task::AbortHandle,
    fingerprint: String,
    status: Arc<Mutex<ServerTunnelStatus>>,
}

/// 隧道任务登记表：每台启用隧道的服务器一个常驻任务。
#[derive(Default)]
pub struct TunnelManager {
    running: Mutex<HashMap<String, RunningTunnel>>,
}

impl TunnelManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 按当前配置对齐：缺的补上、规则变了的重建、规则清空的下线。返回在跑的台数。
    ///
    /// 必须在 tokio 运行时上下文里调用（GUI 的命令与启动任务都满足）。
    pub fn sync(&self, servers: &[ServerConfig]) -> usize {
        let mut running = lock(&self.running);
        let desired: HashMap<String, ServerConfig> = servers
            .iter()
            .filter(|server| !server.active_tunnels().is_empty())
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
            let handle = tokio::spawn(supervise(server, status.clone()));
            running.insert(
                id,
                RunningTunnel {
                    abort: handle.abort_handle(),
                    fingerprint: mark,
                    status,
                },
            );
        }
        running.len()
    }

    /// 强制重建这一台的隧道（配置没变、但用户点了「重新连接」时用）。
    pub fn restart(&self, server: &ServerConfig) -> Result<()> {
        if server.active_tunnels().is_empty() {
            return Err(CoreError::config(format!(
                "服务器「{}」没有启用的隧道规则",
                server.name
            )));
        }
        let mut running = lock(&self.running);
        if let Some(entry) = running.remove(&server.id) {
            entry.abort.abort();
        }
        let status = initial_status(server);
        let handle = tokio::spawn(supervise(server.clone(), status.clone()));
        running.insert(
            server.id.clone(),
            RunningTunnel {
                abort: handle.abort_handle(),
                fingerprint: task_fingerprint(server),
                status,
            },
        );
        Ok(())
    }

    /// 当前所有在跑（或刚失败退出）的隧道状态，按服务器名排序。
    pub fn status(&self) -> Vec<ServerTunnelStatus> {
        let running = lock(&self.running);
        let mut list: Vec<ServerTunnelStatus> = running
            .values()
            .map(|entry| lock(&entry.status).clone())
            .collect();
        list.sort_by(|a, b| a.server_name.cmp(&b.server_name));
        list
    }

    /// 退出时收掉所有监听：本机端口立刻释放。
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

/// 一台隧道任务的初始状态：未连接，规则按启用项列出。
fn initial_status(server: &ServerConfig) -> Arc<Mutex<ServerTunnelStatus>> {
    Arc::new(Mutex::new(ServerTunnelStatus {
        server_id: server.id.clone(),
        server_name: server.name.clone(),
        connected: false,
        retries: 0,
        last_error: None,
        connected_at: None,
        forwarded: 0,
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

/// 一台服务器的隧道主循环：补绑本机端口 -> 连接 -> 转发到会话断开 -> 退避重来。
async fn supervise(server: ServerConfig, status: Arc<Mutex<ServerTunnelStatus>>) {
    let rules = server.active_tunnels();
    let mut bound: Vec<BoundRule> = Vec::with_capacity(rules.len());
    let mut backoff = RECONNECT_MIN_SECS;

    loop {
        // 每一轮都补齐还没绑上的本机端口：启动那会儿端口被别的应用占着、或刚点过「重新连接」
        // 而旧监听还没来得及释放，都不该让这条隧道从此死掉。
        bind_pending_rules(&rules, &mut bound, &status).await;

        let mut held = Duration::ZERO;
        if bound.is_empty() {
            // 一个端口都没占到就不做 SSH 连接，连了也没有东西能转进来。
            with_status(&status, |current| {
                current.connected = false;
                for item in current.rules.iter_mut() {
                    item.active = false;
                }
                current.last_error = Some("本机端口还没空出来，稍后自动重试".to_string());
            });
        } else {
            let started_at = Instant::now();
            let outcome = connect_and_serve(&server, &bound, &status).await;
            held = started_at.elapsed();
            match outcome {
                // 会话正常结束（多为网络中断或 sshd 重启）：单条转发失败留下的错误不覆盖。
                Ok(()) => with_status(&status, |current| {
                    current.connected = false;
                    current.forwarded = 0;
                    for item in current.rules.iter_mut() {
                        item.active = false;
                    }
                    if current.last_error.is_none() {
                        current.last_error = Some("隧道连接已断开，正在重连".to_string());
                    }
                }),
                Err(err) => with_status(&status, |current| {
                    current.connected = false;
                    current.forwarded = 0;
                    current.retries += 1;
                    current.last_error = Some(err.to_string());
                    for item in current.rules.iter_mut() {
                        item.active = false;
                    }
                }),
            }
        }

        // 撑过门槛时长说明不是「一连就断」，退避重新从最小值起。
        if held >= Duration::from_secs(STABLE_SESSION_SECS) {
            backoff = RECONNECT_MIN_SECS;
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(RECONNECT_MAX_SECS);
    }
}

/// 把还没绑上的本机端口补上并写进状态；已绑的沿用，退避间隙不会被别的应用抢走。
async fn bind_pending_rules(
    rules: &[TunnelRule],
    bound: &mut Vec<BoundRule>,
    status: &Arc<Mutex<ServerTunnelStatus>>,
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
                Some(format!("本机端口 {port} 绑定失败: {err}")),
            ),
        }
    }
}

/// 单条规则的监听结果。`active` 只有在会话已在线时才允许点亮，避免没连上也显示「可转发」。
fn mark_rule(status: &Arc<Mutex<ServerTunnelStatus>>, local_port: u16, ok: bool, error: Option<String>) {
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

/// 建立一次连接并转发，直到会话不可用。
async fn connect_and_serve(
    server: &ServerConfig,
    bound: &[BoundRule],
    status: &Arc<Mutex<ServerTunnelStatus>>,
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
        _ => Err(CoreError::ssh("隧道连接已断开")),
    };
    // client 在此处随作用域结束而释放：会话关闭，本机端口仍由 supervise 持有。
    reason
}

/// 接受本机连接并转成一条 direct-tcpip 通道；收到停止信号就退出。
async fn accept_loop(
    client: Arc<SshClient>,
    rule: TunnelRule,
    listener: Arc<TcpListener>,
    mut stop: watch::Receiver<bool>,
    status: Arc<Mutex<ServerTunnelStatus>>,
) -> Result<()> {
    loop {
        let accepted = tokio::select! {
            result = listener.accept() => result,
            _ = stop.changed() => return Ok(()),
        };
        let (stream, _) = accepted.map_err(|err| {
            CoreError::ssh(format!("本机端口 {} 接受连接失败: {err}", rule.local_port))
        })?;

        {
            let mut guard = lock(&status);
            guard.forwarded += 1;
        }

        let client = client.clone();
        let host = rule.remote_host.clone();
        let remote_port = rule.remote_port;
        let counter = status.clone();
        // 每条连接独立一个任务：一条转发结束（或对端失败）不应影响别的连接。
        tokio::spawn(async move {
            if let Err(err) = client.forward_tcp(stream, &host, remote_port).await {
                // 单条连接失败只记账到这台隧道的最近错误：会话还在，用户下次请求就能恢复。
                let mut guard = lock(&counter);
                guard.last_error = Some(err.to_string());
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
            return Err(CoreError::ssh("隧道连接已断开"));
        }
        if tokio::time::timeout(
            Duration::from_secs(HEALTH_CHECK_SECS),
            rx.changed(),
        )
        .await
        .is_ok()
        {
            return Ok(());
        }
    }
}

fn with_status<F>(status: &Arc<Mutex<ServerTunnelStatus>>, f: F)
where
    F: FnOnce(&mut ServerTunnelStatus),
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

    #[test]
    fn normalize_fills_ids_and_keeps_order() {
        let rules = vec![rule(18080, "127.0.0.1", 8080), rule(15432, "localhost", 5432)];
        let out = normalize_rules(&rules).expect("合法规则");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].local_port, 18080);
        assert_eq!(out[1].remote_host, "localhost");
    }

    #[test]
    fn normalize_assigns_id_when_blank() {
        let mut item = rule(18080, "127.0.0.1", 8080);
        item.id = String::new();
        let out = normalize_rules(&[item]).expect("合法规则");
        assert!(!out[0].id.is_empty());
    }

    #[test]
    fn normalize_rejects_blank_host_and_zero_ports() {
        assert!(normalize_rules(&[rule(18080, "  ", 8080)]).is_err());
        assert!(normalize_rules(&[rule(0, "127.0.0.1", 8080)]).is_err());
        assert!(normalize_rules(&[rule(18080, "127.0.0.1", 0)]).is_err());
    }

    #[test]
    fn normalize_rejects_urls_with_scheme_or_port() {
        assert!(normalize_rules(&[rule(18080, "http://a.example", 8080)]).is_err());
        assert!(normalize_rules(&[rule(18080, "127.0.0.1:8080", 8080)]).is_err());
    }

    #[test]
    fn normalize_rejects_duplicate_enabled_local_port() {
        let err = normalize_rules(&[rule(18080, "127.0.0.1", 8080), rule(18080, "127.0.0.1", 9000)])
            .expect_err("同端口应报错");
        assert!(matches!(err, CoreError::Config(_)));
        assert!(err.to_string().contains("18080"));
    }

    #[test]
    fn disabled_rules_may_share_a_port() {
        let mut first = rule(18080, "127.0.0.1", 8080);
        first.enabled = false;
        let second = rule(18080, "127.0.0.1", 9000);
        assert!(normalize_rules(&[first, second]).is_ok());
    }

    #[test]
    fn cross_server_port_conflict_is_reported() {
        let existing = server("a", vec![rule(18080, "127.0.0.1", 8080)]);
        let err = assert_local_ports_free(&[existing.clone()], "b", &[rule(18080, "127.0.0.1", 9000)])
            .expect_err("跨机撞端口");
        assert!(err.to_string().contains("18080"));
        // 改的就是那一台自己的规则不算冲突。
        assert!(assert_local_ports_free(&[existing], "a", &[rule(18080, "127.0.0.1", 9000)]).is_ok());
    }

    #[test]
    fn fingerprint_ignores_disabled_rules() {
        let on = vec![rule(18080, "127.0.0.1", 8080)];
        let mut off = rule(19000, "127.0.0.1", 5432);
        off.enabled = false;
        let mut both = on.clone();
        both.push(off.clone());
        assert_eq!(fingerprint(&on), fingerprint(&both));
        assert_ne!(fingerprint(&on), fingerprint(&[off]));
    }

    async fn free_port() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("找一个空闲端口");
        let port = listener.local_addr().expect("端口号").port();
        drop(listener);
        port
    }

    async fn wait_for<F>(manager: &TunnelManager, mut pred: F) -> Option<ServerTunnelStatus>
    where
        F: FnMut(&ServerTunnelStatus) -> bool,
    {
        // 上限约 8s：覆盖一个 3s 退避周期，端口空出来之后的自愈要等下一轮。
        for _ in 0..320 {
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
    /// 「打开软件就自动接上」可靠的就是这一半；真正转发包要一台活服务器，归手工验证。
    #[tokio::test]
    async fn unreachable_server_still_holds_and_releases_the_local_port() {
        let local = free_port().await;
        let dead = free_port().await;
        let mut target = server("a", vec![rule(local, "127.0.0.1", dead)]);
        target.host = "127.0.0.1".to_string();
        target.port = dead;

        let manager = TunnelManager::new();
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

    /// 绑定结果与错误要成对更新。
    ///
    /// 端口空出来之后旧的「绑定失败」必须清掉，否则界面会一直红着；反过来没连上服务器时
    /// 即便端口绑上了也不许报「可转发」。
    /// 老配置（没有 tunnels 字段）必须照常读得进来，否则升级即配置损坏。
    #[test]
    fn server_config_without_tunnels_field_still_loads() {
        let json = r#"{"id":"a","name":"a","host":"10.0.0.1","port":22,"username":"root",
            "auth":{"type":"password","password":"x"},"defaultTargetDir":""}"#;
        let parsed: ServerConfig = serde_json::from_str(json).expect("旧配置应能反序列化");
        assert!(parsed.tunnels.is_empty());
        assert!(parsed.active_tunnels().is_empty());

        // 规则本身也要能原样往返：界面上存的就是这个形状。
        let with_rule = server("a", vec![rule(18080, "127.0.0.1", 8080)]);
        let text = serde_json::to_string(&with_rule).expect("序列化");
        assert!(text.contains("\"localPort\":18080"), "{text}");
        let back: ServerConfig = serde_json::from_str(&text).expect("读回");
        assert_eq!(back.tunnels, with_rule.tunnels);
    }

    /// Rust 与 TS 是手工镜像的，字段名对不上编译器不会报错、运行时才炸。
    /// 这条测试把界面读到的键名钉死。
    #[test]
    fn status_json_keys_match_the_frontend_types() {
        let status = lock(&initial_status(&server("a", vec![rule(18080, "127.0.0.1", 8080)]))).clone();
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
        ] {
            assert!(value.get(key).is_some(), "缺字段 {key}");
        }
        let item = &value["rules"][0];
        for key in ["ruleId", "localPort", "remoteLabel", "bound", "active", "error"] {
            assert!(item.get(key).is_some(), "缺字段 {key}");
        }
    }

    #[test]
    fn mark_rule_clears_error_when_bound() {
        let status = Arc::new(Mutex::new(ServerTunnelStatus {
            server_id: "a".to_string(),
            server_name: "a".to_string(),
            connected: false,
            retries: 0,
            last_error: None,
            connected_at: None,
            forwarded: 0,
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

    /// 本机端口一时被别的应用占住，不能让这条隧道从此死掉。
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
        let manager = TunnelManager::new();
        assert_eq!(manager.sync(&[target]), 1);

        tokio::time::sleep(Duration::from_millis(200)).await;
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

    #[test]
    fn task_fingerprint_follows_connection_and_enabled_rules() {
        let base = server("a", vec![rule(18080, "127.0.0.1", 8080)]);
        assert_eq!(task_fingerprint(&base), task_fingerprint(&base.clone()));

        // 改名不影响连接，不该把在跑的隧道踢掉重连。
        let mut renamed = base.clone();
        renamed.name = "改了个名字".to_string();
        renamed.default_target_dir = "/srv/other".to_string();
        assert_eq!(task_fingerprint(&base), task_fingerprint(&renamed));

        // 换了口令或主机必须重算，否则老任务会拿旧凭据一直重连。
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
