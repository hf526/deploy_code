use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use deploy_core::{DeployEngine, Store};

/// 正在进行的部署（用于应用退出时清理远端脚本）。
#[derive(Clone)]
pub struct ActiveDeploy {
    pub server_id: String,
    pub target_dir: String,
    /// 部署任务的终止句柄，退出清理前先中止任务，避免它再启动新脚本。
    pub abort: tokio::task::AbortHandle,
}

/// 全局应用状态：所有命令通过它访问配置与部署引擎。
pub struct AppState {
    pub store: Arc<Store>,
    /// 正在监听的文件系统 watcher（按仓库 id 保存），用于 GUI 实时刷新。
    pub watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
    /// 进行中的部署（record_id -> 服务器/目录/任务句柄），退出时用于终止远端脚本。
    pub active_deploys: Mutex<HashMap<String, ActiveDeploy>>,
    /// 部署抢占标记：在 prepare 之前原子占位，避免两个并发命令同时通过检查。
    deploy_claim: AtomicBool,
}

impl AppState {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            watchers: Mutex::new(HashMap::new()),
            active_deploys: Mutex::new(HashMap::new()),
            deploy_claim: AtomicBool::new(false),
        }
    }

    /// 尝试占用部署名额；失败说明已有部署在进行。
    pub fn try_claim_deploy(&self) -> bool {
        self.deploy_claim
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn release_deploy_claim(&self) {
        self.deploy_claim.store(false, Ordering::SeqCst);
    }

    pub fn engine(&self) -> DeployEngine {
        DeployEngine::new(self.store.clone())
    }

    pub fn track_deploy(&self, record_id: &str, deploy: ActiveDeploy) {
        if let Ok(mut deploys) = self.active_deploys.lock() {
            deploys.insert(record_id.to_string(), deploy);
        }
    }

    pub fn untrack_deploy(&self, record_id: &str) {
        if let Ok(mut deploys) = self.active_deploys.lock() {
            deploys.remove(record_id);
        }
    }

    /// 取出并清空当前进行中的部署列表。
    pub fn take_deploys(&self) -> Vec<(String, ActiveDeploy)> {
        match self.active_deploys.lock() {
            Ok(mut deploys) => deploys.drain().collect(),
            Err(_) => Vec::new(),
        }
    }
}
