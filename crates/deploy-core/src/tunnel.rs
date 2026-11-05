//! SSH 隧道规则：本机端口 → 该服务器上的某个地址（等价 `ssh -L`）的语义与校验。
//!
//! 这一层只回答「规则本身合不合法、有没有撞端口、改动要不要重算指纹」，不碰连接：
//! **真正建会话、绑监听、转发与探针的是 [`crate::hostlink`] 里那一条常驻连接**，
//! 每台已登记的服务器一条，会话断了只重连、不退出。
//!
//! 规则存在 `ServerConfig.tunnels` 里（不另存一份配置表），所以改规则就是改服务器配置，
//! 由 `HostLinkManager::sync` 按 `task_fingerprint` 决定要不要重建那一台的任务。

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::models::{ServerConfig, TunnelRule};

/// 一条规则的监听状态（由 [`crate::hostlink`] 填写：本机端口绑没绑上、现在能不能转）。
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

/// 规则集合的指纹：本机端口与远端地址一致就无需重建监听。
pub fn fingerprint(rules: &[TunnelRule]) -> String {
    rules
        .iter()
        .filter(|rule| rule.enabled)
        .map(|rule| format!("{}>{}:{}", rule.local_port, rule.remote_host, rule.remote_port))
        .collect::<Vec<String>>()
        .join(";")
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
}
