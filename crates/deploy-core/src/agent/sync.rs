//! 同步指纹（`agent-sync.json` 的内存形态）与本机侧的陈旧 / 让位判定。

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::models::{AppConfig, Settings};
use crate::store::Store;

/// 上次成功下发到控制机的配置指纹（落盘在 `<数据目录>/agent-sync.json`）。
///
/// 存在的理由只有一个：定时循环要分清「这条配置的让位有没有人接」。执行位写成控制机
/// 却从没下发过（或下发时它还不存在、后来又改了名），控制机上就没有那一份 ——
/// 此时本机若照样让位，这一晚两头都不跑，而且一声不吭。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSyncState {
    /// 指纹是对哪台服务器记的：换了控制机必须重新下发才算数。
    #[serde(default)]
    pub server_id: String,
    /// 下发成功的时刻（展示用，界面告诉你控制机上那份有多旧）。
    #[serde(default)]
    pub synced_at: String,
    #[serde(default)]
    pub backup_config_ids: Vec<String>,
    #[serde(default)]
    pub container_config_ids: Vec<String>,
    /// 下发那一刻的定时备份口径（**本机**写下的值，不是折算到控制机时区之后的 HH:MM）。
    ///
    /// 开关、时间点、选中项都被 `build_bundle` 烘进 config.json，可它们不是「几条配置」，
    /// 所以条数对得上的时候界面一片绿，而控制机照旧跑上次那份：用户关掉的定时没关掉，
    /// 换掉的那条永远不备份。指纹记下这一份，界面与调度才说得出「那份是旧的」。
    #[serde(default)]
    pub backup_schedule: String,
    #[serde(default)]
    pub container_schedule: String,
    /// 已经不是当前控制机、但 agent 没收回来的那台（换控制机时旧那台连不上就会留在这里）。
    /// 它读的是自己盘上那份 config.json，到点照跑，而且那里面有全部源机的明文口令 ——
    /// 所以 [`AgentSyncState::clear`] 不动它，界面上要一直挂着直到收回成功。
    #[serde(default)]
    pub orphan_server_ids: Vec<String>,
}

impl AgentSyncState {
    /// 记一台待收回的旧控制机（去重）。
    pub fn add_orphan(&mut self, server_id: &str) {
        let id = server_id.trim();
        if !id.is_empty() && !self.orphan_server_ids.iter().any(|item| item == id) {
            self.orphan_server_ids.push(id.to_string());
        }
    }

    /// 收回成功（或那台服务器被删掉）之后撤掉这条待办。
    pub fn remove_orphan(&mut self, server_id: &str) {
        self.orphan_server_ids.retain(|item| item != server_id);
    }
    /// 这份指纹是不是给当前那台控制机记的。
    pub fn is_for(&self, server_id: &str) -> bool {
        !self.server_id.trim().is_empty() && self.server_id == server_id
    }

    pub fn holds_backup(&self, server_id: &str, config_id: &str) -> bool {
        self.is_for(server_id) && self.backup_config_ids.iter().any(|id| id == config_id)
    }

    pub fn holds_container(&self, server_id: &str, config_id: &str) -> bool {
        self.is_for(server_id) && self.container_config_ids.iter().any(|id| id == config_id)
    }

    /// 控制机上那份定时（数据库备份）与本机当前设置不一致。
    ///
    /// 从没下发过（`backup_schedule` 是空）不算「陈旧」而是「未下发」，由
    /// [`AgentSyncState::holds_backup`] 那条路报错，两头不重复提醒。
    pub fn backup_schedule_stale(&self, server_id: &str, key: &str) -> bool {
        self.is_for(server_id) && !self.backup_schedule.is_empty() && self.backup_schedule != key
    }

    pub fn container_schedule_stale(&self, server_id: &str, key: &str) -> bool {
        self.is_for(server_id) && !self.container_schedule.is_empty() && self.container_schedule != key
    }

    /// 控制机还持有、本机却已经不再交给它的配置名（执行位改回本机，或配置已被删除）。
    ///
    /// 那台机器读的是自己盘上的 config.json，不会自己停：不补一次下发，改回本机的那条
    /// 今晚两边各跑一次（两份包落在两台机器上，而跨机没有任务锁），被删掉的那条则继续
    /// 每晚替一个本机已经没有的配置导出。
    pub fn stragglers(&self, config: &AppConfig) -> Vec<String> {
        let mut names = Vec::new();
        for id in &self.backup_config_ids {
            match config.backup_configs.iter().find(|item| &item.id == id) {
                None => names.push(format!("已删除的备份配置 {id}")),
                Some(item) if !item.run_location.is_remote() => {
                    names.push(format!("「{}」已改回本机", item.name))
                }
                Some(_) => {}
            }
        }
        for id in &self.container_config_ids {
            match config.container_configs.iter().find(|item| &item.id == id) {
                None => names.push(format!("已删除的容器配置 {id}")),
                Some(item) if !item.run_location.is_remote() => {
                    names.push(format!("「{}」已改回本机", item.name))
                }
                Some(_) => {}
            }
        }
        names
    }

    /// 清空（卸载时调用）：留着一个指向不存在的服务器的指纹，比没有指纹更容易骗过让位判定。
    ///
    /// `orphan_server_ids` 不在清空范围内：它不是指纹，而是「还有哪台机器上没收回 agent」的待办，
    /// 卸载/换机把现役指纹清掉时那份待办得留着，否则旧控制机就悄悄没人管了。
    pub fn clear(&mut self) {
        self.server_id = String::new();
        self.synced_at = String::new();
        self.backup_config_ids = Vec::new();
        self.container_config_ids = Vec::new();
        self.backup_schedule = String::new();
        self.container_schedule = String::new();
    }
}

/// 下发那一刻的定时备份口径（本机值）。参与指纹比较，不参与折算。
pub fn backup_schedule_key(config: &AppConfig) -> String {
    format!(
        "{}|{}|{}",
        config.settings.scheduled_backup_enabled,
        config.settings.scheduled_backup_time.trim(),
        config
            .settings
            .scheduled_backup_config_id
            .as_deref()
            .unwrap_or("")
            .trim()
    )
}

/// 容器那边同理，只是选中项是一个列表（顺序即当晚执行顺序）。
pub fn container_schedule_key(config: &AppConfig) -> String {
    format!(
        "{}|{}|{}",
        config.settings.scheduled_container_enabled,
        config.settings.scheduled_container_time.trim(),
        config
            .settings
            .scheduled_container_config_ids
            .iter()
            .map(|id| id.trim())
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// 「控制机上那份与本机设置已经不一致」的本机视图：纯读盘，不连服务器。
///
/// 存在的理由：`syncNeeded` 只比配置条数，看不出定时设置的改动，也看不出某条配置被改回
/// 本机 / 被删掉之后控制机还留着它那一半。这两类都会让控制机在夜里做出与界面相反的事。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStaleness {
    /// 定时的数据库备份改动还没下发（控制机上跑的是上次那份）。
    pub backup_schedule_stale: bool,
    /// 定时的容器备份改动还没下发。
    pub container_schedule_stale: bool,
    /// 控制机还持有、本机已不再交给它的配置（点名是谁）。
    pub stragglers: Vec<String>,
    /// 上次成功下发的时刻；空串 = 从没下发过。
    pub synced_at: String,
}

/// 只读本机的 config.json 与 agent-sync.json，供界面与调度提醒用。
pub fn staleness(store: &Store) -> AgentStaleness {
    let Ok(config) = store.load_config() else {
        // 读不到本机配置就当没有可提醒的：这一栏是提示，不该把设置页拖成报错。
        return AgentStaleness::default();
    };
    let sync = store.load_agent_sync();
    let server_id = config.settings.agent_server_id.trim();
    if !sync.is_for(server_id) {
        // 指纹不是当前这台控制机的（没装、换过机器、刚卸载）：那时是「未下发」，
        // 由让位判定报失败，这里不重复报陈旧。
        return AgentStaleness::default();
    }
    AgentStaleness {
        backup_schedule_stale: sync.backup_schedule_stale(server_id, &backup_schedule_key(&config)),
        container_schedule_stale: sync
            .container_schedule_stale(server_id, &container_schedule_key(&config)),
        stragglers: sync.stragglers(&config),
        synced_at: sync.synced_at.clone(),
    }
}

/// 控制机是哪台：`settings.agent_server_id`。
pub fn agent_server_id(settings: &Settings) -> Result<String> {
    let id = settings.agent_server_id.trim();
    if id.is_empty() {
        return Err(CoreError::config("还没有指定控制机，请先在控制机页面选择一台服务器"));
    }
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::agent::build_bundle;
    use crate::agent::control::AgentControl;
    use crate::agent::testutil::{backup_config, container_config, tempfile};

    #[test]
    fn sync_state_only_answers_for_the_machine_it_was_recorded_on() {
        let mut state = AgentSyncState {
            server_id: "s1".to_string(),
            synced_at: "2026-09-27 03:00:00".to_string(),
            backup_config_ids: vec!["b1".to_string()],
            container_config_ids: vec!["c1".to_string()],
            ..Default::default()
        };
        assert!(state.holds_backup("s1", "b1"));
        assert!(state.holds_container("s1", "c1"));
        // 换了控制机（或从来没同步过）就不能让位：那台机器上没有这一份。
        assert!(!state.holds_backup("s2", "b1"));
        assert!(!state.holds_backup("s1", "b9"));
        assert!(!AgentSyncState::default().holds_backup("s1", "b1"));

        // 待收回那份是指纹之外的东西：作废指纹（卸载 / 换机）不该把「旧那台还没收回」一起清掉。
        state.add_orphan("s0");
        state.add_orphan("s0");
        assert_eq!(state.orphan_server_ids, vec!["s0".to_string()]);
        state.clear();
        assert!(state.server_id.is_empty() && state.backup_config_ids.is_empty());
        assert_eq!(state.orphan_server_ids, vec!["s0".to_string()]);
        state.remove_orphan("s0");
        assert!(state.orphan_server_ids.is_empty());
    }

    #[test]
    fn agent_server_id_requires_a_choice() {
        let settings = Settings::default();
        assert!(agent_server_id(&settings).is_err());
        let settings = Settings {
            agent_server_id: " s1 ".to_string(),
            ..Default::default()
        };
        assert_eq!(agent_server_id(&settings).unwrap(), "s1");
    }

    /// 临时目录（deploy-core 的测试没有引 tempfile crate，这里用最直白的方式）。
    /// 条数一样但内容已经变了：这一类改动以前两头都看不见（`syncNeeded` 只比条数），
    /// 而控制机那一晚会跑出与界面上相反的东西。
    #[test]
    fn staleness_names_schedule_changes_and_configs_the_agent_still_holds() {
        let dir = tempfile();
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        config.settings.agent_server_id = "s1".to_string();
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_time = "03:00".to_string();
        config.settings.scheduled_backup_config_id = Some("b1".to_string());
        config.settings.scheduled_container_enabled = true;
        config.settings.scheduled_container_time = "04:00".to_string();
        config.settings.scheduled_container_config_ids = vec!["c1".to_string()];
        config.backup_configs = vec![backup_config("b1", "夜间库", true)];
        config.container_configs = vec![container_config("c1", "远端项目", true)];
        store.save_config(&config).unwrap();

        // 下发那一刻：指纹由 remember_sync 写，两边都从它取，口径不会分叉。
        let bundle = build_bundle(&config, Some(480));
        let control = AgentControl::new(Arc::new(Store::new(&dir)));
        control
            .remember_sync("s1", &bundle, &store.load_config().unwrap())
            .expect("写指纹");
        let same = staleness(&store);
        assert!(
            !same.backup_schedule_stale && !same.container_schedule_stale,
            "一模一样不该报陈旧：{same:?}"
        );
        assert!(same.stragglers.is_empty() && !same.synced_at.is_empty());

        // 只改时间点：配置条数不变，界面上那个「与本机不一致」以前根本亮不起来。
        let mut moved = config.clone();
        moved.settings.scheduled_backup_time = "11:00".to_string();
        store.save_config(&moved).unwrap();
        let stale = staleness(&store);
        assert!(stale.backup_schedule_stale, "改了时间要报陈旧：{stale:?}");
        assert!(!stale.container_schedule_stale);

        // 执行位改回本机、以及控制机还持有的容器配置被删掉：两份都要点名。
        let mut revoked = moved.clone();
        revoked.backup_configs = vec![backup_config("b1", "夜间库", false)];
        revoked.container_configs = Vec::new();
        store.save_config(&revoked).unwrap();
        let names = staleness(&store).stragglers;
        assert!(
            names.iter().any(|item| item.contains("夜间库"))
                && names.iter().any(|item| item.contains("c1")),
            "改回本机与被删的都该点出来：{names:?}"
        );

        // 从没下发过 / 换了指向：那是「未下发」，由让位判定报失败，这里不重复报陈旧。
        let mut other = revoked.clone();
        other.settings.agent_server_id = "s9".to_string();
        store.save_config(&other).unwrap();
        let none = staleness(&store);
        assert!(
            !none.backup_schedule_stale && none.stragglers.is_empty(),
            "指纹不属于当前控制机时不该报陈旧：{none:?}"
        );
    }
}
