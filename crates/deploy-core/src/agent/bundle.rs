//! 下发配置子集（bundle）的构建与下发前的一致性检查。

use crate::models::{AppConfig, Settings};

use super::AGENT_DB_BUNDLE_KEEP;

/// 解析 `date +%z` 那类输出里的 UTC 偏移，返回分钟数（东为正）：`+0800` → 480。
///
/// 容忍前后有杂项（登录 shell 的欢迎语、`\r`）：只认第一个形如 `±HHMM` 的片段。
pub(super) fn parse_utc_offset(text: &str) -> Option<i32> {
    for (index, byte) in text.as_bytes().iter().enumerate() {
        if !matches!(byte, b'+' | b'-') {
            continue;
        }
        if let Some(offset) = offset_after(text, index, *byte) {
            return Some(offset);
        }
    }
    None
}

/// 从 `index` 处那个 +/- 开始读偏移：`+0800`、`+08:00`、`+8` 都认；认不出来返回 None，
/// 让外层继续往后找（前面可能是 shell 打的无关字符）。
fn offset_after(text: &str, index: usize, sign: u8) -> Option<i32> {
    let tail = &text[index + 1..];
    let run: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    let (hour, minute) = if run.len() >= 3 {
        // `+HHMM`：末两位是分。
        (run[..run.len() - 2].parse().ok()?, run[run.len() - 2..].parse().ok()?)
    } else {
        // `+HH` 或 `+HH:MM`。
        let hours: i32 = run.parse().ok()?;
        let after = tail[run.len()..].strip_prefix(':').unwrap_or("");
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let minutes: i32 = if digits.is_empty() { 0 } else { digits.parse().ok()? };
        (hours, minutes)
    };
    if hour > 14 || minute > 59 {
        return None;
    }
    let value = hour * 60 + minute;
    Some(if sign == b'-' { -value } else { value })
}

/// 从整份配置里挑出 agent 需要的那部分。
///
/// 过去的是「执行备份所需的最小集合」：服务器（含 SSH 凭据）、备份目标、
/// 执行位为远端的备份/容器配置，以及与定时相关的那几项设置。
/// 仓库、部署配置、Pages 配置、各类 Token、主密码一概不过去。
///
/// `agent_offset_minutes` 是控制机相对 UTC 的偏移（分钟，东为正），读不到时传 None：
/// 那样就原样下发，等于按控制机的时区解释用户写在本机时区里的时间点，
/// 而 [`bundle_warnings`] 会把「没折算」这件事讲出来 —— 静默地把备份挪到业务高峰是不能接受的。
pub fn build_bundle(config: &AppConfig, agent_offset_minutes: Option<i32>) -> AppConfig {
    let mut bundle = AppConfig::default();
    bundle.servers = config.servers.clone();
    bundle.backup_targets = config.backup_targets.clone();
    bundle.backup_configs = config
        .backup_configs
        .iter()
        .filter(|item| item.run_location.is_remote())
        .cloned()
        .collect();
    bundle.container_configs = config
        .container_configs
        .iter()
        .filter(|item| item.run_location.is_remote())
        .cloned()
        .collect();

    let local = &config.settings;
    let mut settings = Settings::default();
    settings.connect_timeout_secs = local.connect_timeout_secs;
    settings.script_timeout_secs = local.script_timeout_secs;
    settings.backup_timeout_secs = local.backup_timeout_secs;
    settings.container_timeout_secs = local.container_timeout_secs;
    settings.backup_history_limit = local.backup_history_limit;
    settings.container_history_limit = local.container_history_limit;
    // 本机默认不留导出包（`db_bundle_keep` = 0，与加这个功能之前一致），但控制机必须留：
    // 不留就没有可恢复的产物，「恢复到另一台」和轮转都无从谈起。0 在这里的含义是
    // 「别烦我本机」，不是「控制机也别留」。
    settings.db_bundle_keep = if local.db_bundle_keep == 0 {
        AGENT_DB_BUNDLE_KEEP
    } else {
        local.db_bundle_keep
    };
    settings.container_bundle_keep = local.container_bundle_keep;
    settings.supabase_url = local.supabase_url.clone();
    settings.default_backup_target_id = local.default_backup_target_id.clone();
    settings.scheduled_container_enabled = local.scheduled_container_enabled;
    settings.scheduled_container_time =
        shift_for_agent(&local.scheduled_container_time, agent_offset_minutes)
            .unwrap_or_else(|| local.scheduled_container_time.clone());
    settings.scheduled_container_config_ids = local
        .scheduled_container_config_ids
        .iter()
        .filter(|id| {
            bundle
                .container_configs
                .iter()
                .any(|item| &item.id == *id && !item.name.trim().is_empty())
        })
        .cloned()
        .collect();
    // 定时数据库备份只在「选中的那条配置是远端执行」时才下发，否则两边都会以为对方负责，
    // 结果是这一晚谁都不跑。
    settings.scheduled_backup_enabled = local.scheduled_backup_enabled
        && local
            .scheduled_backup_config_id
            .as_deref()
            .is_some_and(|id| bundle.backup_configs.iter().any(|item| item.id == id));
    settings.scheduled_backup_config_id = settings
        .scheduled_backup_enabled
        .then(|| local.scheduled_backup_config_id.clone())
        .flatten();
    settings.scheduled_backup_time =
        shift_for_agent(&local.scheduled_backup_time, agent_offset_minutes)
            .unwrap_or_else(|| local.scheduled_backup_time.clone());
    settings.agent_server_id = local.agent_server_id.clone();
    bundle.settings = settings;
    bundle
}

/// 把一个「本机时区的墙上时刻」平移成控制机当地的墙上时刻（按它下一次发生时的本机偏移算）。
///
/// 返回 None 表示没能折算：时间点写法不对，或拿不到控制机偏移。调用方按原样下发。
fn shift_for_agent(value: &str, agent_offset_minutes: Option<i32>) -> Option<String> {
    let agent_offset = agent_offset_minutes?;
    let local_offset = crate::schedule::local_offset_at_next(value, &chrono::Local::now())?;
    crate::schedule::shift_hhmm(value, i64::from(agent_offset - local_offset))
}

/// 时间点被平移过时给用户的说法；没平移（同区、或压根没折算成）就不吭声。
fn shifted_note(label: &str, local: &str, agent: &str) -> Option<String> {
    let local = local.trim();
    let agent = agent.trim();
    (local != agent && !local.is_empty() && !agent.is_empty()).then(|| {
        format!("{label}的 {local}（本机时区）已折算成控制机当地的 {agent}。")
    })
}

/// 下发前的一致性检查：把「跑不起来但不算错误」的情况先讲清楚。
///
/// `agent_offset_minutes` 要传进来是因为它决定时间点有没有被折算（见 [`build_bundle`]）：
/// 折算过、以及想折算却没读到偏移，两种情况都得让用户看见，否则他会以为控制机跑的是自己设的那个点。
pub fn bundle_warnings(
    config: &AppConfig,
    bundle: &AppConfig,
    agent_offset_minutes: Option<i32>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let local = &config.settings;
    if local.scheduled_backup_enabled && !bundle.settings.scheduled_backup_enabled {
        if let Some(id) = local.scheduled_backup_config_id.as_deref() {
            let name = config
                .backup_configs
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.name.clone())
                .unwrap_or_else(|| id.to_string());
            warnings.push(format!(
                "定时备份选的是「{name}」，它的执行位是本机，控制机不会接手这项定时。"
            ));
        }
    }
    let skipped = local
        .scheduled_container_config_ids
        .iter()
        .filter(|id| {
            !bundle
                .settings
                .scheduled_container_config_ids
                .iter()
                .any(|keep| keep == *id)
        })
        .count();
    if local.scheduled_container_enabled && skipped > 0 {
        warnings.push(format!(
            "容器定时里有 {skipped} 条配置的执行位是本机，控制机不会接手它们。"
        ));
    }
    if bundle.backup_configs.is_empty() && bundle.container_configs.is_empty() {
        warnings.push("当前没有任何配置的执行位是「控制机」，下发过去只有服务器凭据，不会自动跑备份。".to_string());
    }
    // 交给控制机的配置必须自带目标：`resolve_backup` 不许它兜底到服务器绑定 / 全局默认（那等于换库，
    // 见 backup.rs），所以这种配置到点必失败。下发时就点名，别让用户从当晚的失败记录里倒推。
    let ownerless: Vec<String> = bundle
        .backup_configs
        .iter()
        .filter(|item| {
            let has_target = item
                .target_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            let has_url = item
                .supabase_url
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
            !(has_target || has_url)
        })
        .map(|item| item.name.clone())
        .collect();
    if !ownerless.is_empty() {
        warnings.push(format!(
            "控制机上的备份配置「{}」没有自己的备份目标，到点会失败（不会退到服务器绑定或全局默认，那等于换库）。请在数据库备份页为它选一个目标再下发。",
            ownerless.join("」「")
        ));
    }
    // 时间点折算过就要说出来：控制机的钟和本机的钟不一样时，用户设的 03:00 会落在别的时刻。
    if let Some(note) = shifted_note(
        "定时数据库备份",
        &local.scheduled_backup_time,
        &bundle.settings.scheduled_backup_time,
    ) {
        warnings.push(note);
    }
    if let Some(note) = shifted_note(
        "定时容器备份",
        &local.scheduled_container_time,
        &bundle.settings.scheduled_container_time,
    ) {
        warnings.push(note);
    }
    if agent_offset_minutes.is_none()
        && (bundle.settings.scheduled_backup_enabled || bundle.settings.scheduled_container_enabled)
    {
        // 只有一种情况需要提醒：读不到控制机时区，于是两个时间点都是照原样发过去的。
        warnings.push("没读到控制机的时区，定时时间按控制机当地解释；若两机时区不同，实际时刻会与设置里不同。".to_string());
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::testutil::{backup_config, container_config, server};
    use crate::models::BackupTarget;

    #[test]
    fn bundle_keeps_only_remote_configs_and_credentials() {
        let mut config = AppConfig::default();
        config.servers = vec![server("s1", "源机"), server("s2", "目标机")];
        config.backup_configs = vec![
            backup_config("b1", "远端库", true),
            backup_config("b2", "本机库", false),
        ];
        config.container_configs = vec![
            container_config("c1", "远端项目", true),
            container_config("c2", "本机项目", false),
        ];
        config.settings.github_token = "ghp_secret".to_string();
        config.settings.cloudflare_api_token = "cf_secret".to_string();
        config.settings.master_password_hash = Some("hash".to_string());
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_time = "02:00".to_string();
        config.settings.scheduled_backup_config_id = Some("b1".to_string());
        config.settings.scheduled_container_enabled = true;
        config.settings.scheduled_container_config_ids = vec!["c1".to_string(), "c2".to_string()];
        config.settings.scheduled_backup_last_run = "2026-09-27".to_string();

        let bundle = build_bundle(&config, None);
        assert_eq!(bundle.backup_configs.len(), 1);
        assert_eq!(bundle.backup_configs[0].id, "b1");
        assert_eq!(bundle.container_configs.len(), 1);
        assert_eq!(bundle.container_configs[0].id, "c1");
        // 定时队列里剔掉本机执行的那条，否则控制机会替它再跑一遍。
        assert_eq!(bundle.settings.scheduled_container_config_ids, vec!["c1".to_string()]);
        assert!(bundle.settings.scheduled_backup_enabled);
        // 拿不到控制机时区 → 原样下发（bundle_warnings 会说明这一点）。
        assert_eq!(bundle.settings.scheduled_backup_time, "02:00");
        // 本机那份是不留包的（0），但控制机必须留，否则远端执行没有产物。
        assert_eq!(bundle.settings.db_bundle_keep, AGENT_DB_BUNDLE_KEEP);
        // 凭据下发只覆盖备份要用的部分。
        assert_eq!(bundle.servers.len(), 2);
        assert!(bundle.settings.github_token.is_empty());
        assert!(bundle.settings.cloudflare_api_token.is_empty());
        assert!(bundle.settings.master_password_hash.is_none());
        // agent 自己的触发日期不能被顶掉（它记在 schedule-state.json）。
        assert!(bundle.settings.scheduled_backup_last_run.is_empty());
        assert!(bundle.repos.is_empty());
        assert!(bundle.deploy_configs.is_empty());
    }

    #[test]
    fn bundle_drops_backup_schedule_pointing_at_a_local_config() {
        let mut config = AppConfig::default();
        config.backup_configs = vec![backup_config("b2", "本机库", false)];
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_config_id = Some("b2".to_string());
        let bundle = build_bundle(&config, None);
        assert!(!bundle.settings.scheduled_backup_enabled);
        assert!(bundle.settings.scheduled_backup_config_id.is_none());

        let warnings = bundle_warnings(&config, &bundle, None);
        assert!(
            warnings.iter().any(|item| item.contains("本机库")),
            "{warnings:?}"
        );
    }

    /// 执行位交给控制机、却没给自己留备份目标的配置：控制机到点必失败（不许兜底换库），
    /// 所以下发那一刻就要点名，而不是让用户从当晚的失败记录里倒推。
    #[test]
    fn bundle_warns_about_remote_configs_without_a_target() {
        let mut config = AppConfig::default();
        let mut with_target = backup_config("b1", "带目标", true);
        with_target.target_id = Some("t1".to_string());
        let mut with_url = backup_config("b2", "带连接串", true);
        with_url.supabase_url = Some("postgres://u:p@h/db".to_string());
        config.backup_configs = vec![with_target, with_url, backup_config("b3", "裸配置", true)];
        config.backup_targets = vec![BackupTarget::new(
            "目标库".to_string(),
            "postgres://u:p@h/db".to_string(),
        )];

        let warnings = bundle_warnings(&config, &build_bundle(&config, None), None);
        let flagged: Vec<&String> = warnings
            .iter()
            .filter(|item| item.contains("没有自己的备份目标"))
            .collect();
        assert_eq!(flagged.len(), 1, "{warnings:?}");
        // 只点名缺目标的那条：另两条各有自己的来源，不该被牵连。
        assert!(
            flagged[0].contains("裸配置")
                && !flagged[0].contains("带目标")
                && !flagged[0].contains("带连接串"),
            "{flagged:?}"
        );
    }

    #[test]
    fn bundle_keeps_the_users_own_db_bundle_keep_when_set() {
        let mut config = AppConfig::default();
        config.settings.db_bundle_keep = 5;
        let bundle = build_bundle(&config, None);
        // 兜底只兜 0：用户显式设过的份数要照发过去。
        assert_eq!(bundle.settings.db_bundle_keep, 5);
    }

    #[test]
    fn bundle_shifts_schedule_to_the_agents_wall_clock() {
        let mut config = AppConfig::default();
        config.backup_configs = vec![backup_config("b1", "夜间库", true)];
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_config_id = Some("b1".to_string());
        config.settings.scheduled_backup_time = "03:00".to_string();

        // 本机偏移是环境相关的（测试跑在哪台机器上不知道），所以断言差值而不是绝对值：
        // 控制机比本机东边 5 小时，控制机那份时间点就该写成 08:00（同一瞬间）。
        let local = crate::schedule::local_offset_at_next("03:00", &chrono::Local::now()).unwrap();
        let bundle = build_bundle(&config, Some(local + 5 * 60));
        assert_eq!(bundle.settings.scheduled_backup_time, "08:00");
        // 同区时不平移。
        let same = build_bundle(&config, Some(local));
        assert_eq!(same.settings.scheduled_backup_time, "03:00");
        // 折算过就要在同步结果里说清楚，不能让用户以为控制机跑的是 03:00。
        let warnings = bundle_warnings(&config, &bundle, Some(local + 5 * 60));
        assert!(
            warnings.iter().any(|item| item.contains("03:00") && item.contains("08:00")),
            "{warnings:?}"
        );
        // 读不到偏移时也要说：那时候时间点没动，但两机可能并不同时区。
        let blind = bundle_warnings(&config, &build_bundle(&config, None), None);
        assert!(blind.iter().any(|item| item.contains("时区")), "{blind:?}");
    }

    #[test]
    fn parse_utc_offset_reads_date_plus_z() {
        assert_eq!(parse_utc_offset("+0800\n"), Some(480));
        assert_eq!(parse_utc_offset("-0330"), Some(-210));
        assert_eq!(parse_utc_offset("+08:00"), Some(480));
        assert_eq!(parse_utc_offset("+8"), Some(480));
        // 登录 shell 的杂项输出前面有别的正负号也要能认出来。
        assert_eq!(parse_utc_offset("welcome\ncwd: /var/tmp\n+0800"), Some(480));
        assert_eq!(parse_utc_offset("GMT"), None);
        assert_eq!(parse_utc_offset(""), None);
        // 不存在的偏移不当数：认错了会把定时挪到完全错误的时刻。
        assert_eq!(parse_utc_offset("+9900"), None);
    }
}
