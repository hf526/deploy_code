use deploy_core::models::{
    Branch, Commit, EnvFileConfig, FileContent, FileEntry, GraphCommit, RepoConfig, RepoInfo,
    RepoStatus, ReplaceSummary, ResolvedRev, SearchHit,
};
use deploy_core::{engine::normalize_env_files, repo_info, CoreError, Git, Result, Store};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDetail {
    pub repo: RepoInfo,
    pub status: Option<RepoStatus>,
}

fn get_repo(state: &State<AppState>, repo_id: &str) -> Result<RepoConfig> {
    let config = state.store.load_config()?;
    Ok(Store::find_repo(&config, repo_id)?.clone())
}

fn git_for(state: &State<AppState>, repo_id: &str) -> Result<Git> {
    let repo = get_repo(state, repo_id)?;
    Git::open(&repo.path)
}

#[tauri::command(async)]
pub fn list_repos(state: State<AppState>) -> Result<Vec<RepoInfo>> {
    let config = state.store.load_config()?;
    Ok(config.repos.iter().map(repo_info).collect())
}

#[tauri::command(async)]
pub fn repo_detail(state: State<AppState>, repo_id: String) -> Result<RepoDetail> {
    let repo = get_repo(&state, &repo_id)?;
    let info = repo_info(&repo);
    let status = Git::open(&repo.path).ok().and_then(|git| git.status().ok());
    Ok(RepoDetail { repo: info, status })
}

#[tauri::command(async)]
pub fn add_repo(
    state: State<AppState>,
    path: String,
    name: Option<String>,
    default_server_id: Option<String>,
    default_target_dir: Option<String>,
) -> Result<RepoInfo> {
    let path_buf = deploy_core::process::canonicalize_path(std::path::Path::new(&path));
    register_repo(&state, path_buf, name, default_server_id, default_target_dir)
}

/// 从远端克隆仓库到本地并登记为可用仓库。
#[tauri::command(async)]
pub fn clone_repo(
    state: State<AppState>,
    url: String,
    parent_dir: String,
    name: Option<String>,
) -> Result<RepoInfo> {
    let parent = parent_dir.trim();
    if parent.is_empty() {
        return Err(CoreError::config("请选择克隆到的目录"));
    }
    let parent_buf = deploy_core::process::canonicalize_path(std::path::Path::new(parent));
    if !parent_buf.is_dir() {
        return Err(CoreError::config(format!(
            "目录不存在: {}",
            parent_buf.display()
        )));
    }

    let dir_name = name
        .as_deref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| derive_repo_name(&url))
        .ok_or_else(|| CoreError::config("无法从地址推断仓库名，请填写目录名"))?;
    if dir_name == "."
        || dir_name == ".."
        || dir_name.contains(['/', '\\'])
        || dir_name.contains(':')
    {
        return Err(CoreError::config("仓库目录名不合法"));
    }

    let dest = parent_buf.join(&dir_name);
    let existed = dest.exists();
    if existed {
        let mut entries =
            std::fs::read_dir(&dest).map_err(|e| CoreError::io_path(&dest, e))?;
        if entries.next().is_some() {
            return Err(CoreError::config(format!(
                "目标目录已存在且非空: {}",
                dest.display()
            )));
        }
    }

    if let Err(err) = Git::clone(&url, &dest) {
        // 克隆失败时清掉残留内容，避免用户重试被“目录非空”挡住；
        // 目录原本就存在时保留一个空目录，不删除用户的目录本身。
        let _ = std::fs::remove_dir_all(&dest);
        if existed {
            let _ = std::fs::create_dir_all(&dest);
        }
        return Err(err);
    }
    register_repo(&state, dest, name, None, None)
}

fn derive_repo_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next()?;
    let name = last.trim_end_matches(".git");
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// 登记一个本地文件夹（已存在时直接复用），返回其运行时信息；不要求已经是 Git 仓库。
fn register_repo(
    state: &State<AppState>,
    path_buf: std::path::PathBuf,
    name: Option<String>,
    default_server_id: Option<String>,
    default_target_dir: Option<String>,
) -> Result<RepoInfo> {
    let normalized = path_buf.to_string_lossy().replace('\\', "/");

    // IDE 式“打开文件夹”语义：已经打开过就直接复用，不报错。
    {
        let config = state.store.load_config()?;
        if let Some(existing) = config.repos.iter().find(|repo| {
            repo.path.replace('\\', "/").trim_end_matches('/') == normalized.trim_end_matches('/')
        }) {
            return Ok(repo_info(existing));
        }
    }

    // 只校验目录存在，不要求已经是 Git 仓库：非 Git 文件夹也可以在绑定远端时自动初始化。
    if !path_buf.is_dir() {
        return Err(CoreError::not_found(format!(
            "目录不存在: {}",
            path_buf.display()
        )));
    }

    let base = name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            path_buf
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "repo".to_string());

    let repo = state.store.mutate_config(|config| {
        if let Some(existing) = config.repos.iter().find(|repo| {
            repo.path.replace('\\', "/").trim_end_matches('/') == normalized.trim_end_matches('/')
        }) {
            return Ok(existing.clone());
        }
        // 同名自动加序号，避免打开重名目录时报错。
        let mut name = base.clone();
        let mut seq = 2;
        while config.repos.iter().any(|repo| repo.name == name) {
            name = format!("{base}-{seq}");
            seq += 1;
        }
        let mut repo = RepoConfig::new(name, path_buf.to_string_lossy().into_owned());
        // 防止保存悬空引用：默认服务器必须存在（空值视为未设置）。
        repo.default_server_id = match default_server_id.filter(|value| !value.trim().is_empty()) {
            Some(value) => {
                let value = value.trim().to_string();
                config
                    .servers
                    .iter()
                    .find(|server| server.id == value || server.name == value)
                    .ok_or_else(|| CoreError::not_found(format!("服务器不存在: {value}")))?;
                Some(value)
            }
            None => None,
        };
        repo.default_target_dir = default_target_dir.unwrap_or_default();
        config.repos.push(repo.clone());
        Ok(repo)
    })?;
    Ok(repo_info(&repo))
}

#[tauri::command(async)]
pub fn update_repo(
    state: State<AppState>,
    repo_id: String,
    name: Option<String>,
    default_server_id: Option<String>,
    default_target_dir: Option<String>,
) -> Result<RepoInfo> {
    let next_name = name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let updated = state.store.mutate_config(|config| {
        if let Some(name) = &next_name {
            if config
                .repos
                .iter()
                .any(|item| item.id != repo_id && item.name == *name)
            {
                return Err(CoreError::config(format!("仓库名称已存在: {name}")));
            }
        }

        let repo = config
            .repos
            .iter_mut()
            .find(|repo| repo.id == repo_id)
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {repo_id}")))?;

        if let Some(name) = next_name {
            repo.name = name;
        }
        // None 表示“本次不修改”，空串才表示清空，避免重命名等局部更新把默认配置抹掉。
        if let Some(value) = default_server_id {
            let value = value.trim().to_string();
            if value.is_empty() {
                repo.default_server_id = None;
            } else {
                // 防止保存悬空引用：默认服务器必须存在。
                config
                    .servers
                    .iter()
                    .find(|server| server.id == value || server.name == value)
                    .ok_or_else(|| CoreError::not_found(format!("服务器不存在: {value}")))?;
                repo.default_server_id = Some(value);
            }
        }
        if let Some(value) = default_target_dir {
            repo.default_target_dir = value.trim().to_string();
        }
        Ok(repo.clone())
    })?;
    Ok(repo_info(&updated))
}

#[tauri::command(async)]
pub fn save_repo_env_files(
    state: State<AppState>,
    repo_id: String,
    env_files: Vec<EnvFileConfig>,
) -> Result<RepoInfo> {
    let normalized = normalize_env_files(&env_files)?;
    state.store.mutate_config(|config| {
        let repo = config
            .repos
            .iter_mut()
            .find(|repo| repo.id == repo_id)
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {repo_id}")))?;
        repo.env_files = normalized.clone();
        Ok(())
    })?;
    let config = state.store.load_config()?;
    Ok(repo_info(Store::find_repo(&config, &repo_id)?))
}

#[tauri::command(async)]
pub fn remove_repo(state: State<AppState>, repo_id: String) -> Result<()> {
    state.store.mutate_config(|config| {
        config.repos.retain(|repo| repo.id != repo_id);
        Ok(())
    })?;
    // 仓库移除后停止文件监听，避免 watcher 继续占用资源。
    state
        .watchers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&repo_id);
    Ok(())
}

#[tauri::command(async)]
pub fn set_repo_remote(state: State<AppState>, repo_id: String, url: String) -> Result<RepoInfo> {
    let repo = get_repo(&state, &repo_id)?;
    Git::open(&repo.path)?.set_remote_url(&url)?;
    Ok(repo_info(&repo))
}

#[tauri::command(async)]
pub fn list_branches(
    state: State<AppState>,
    repo_id: String,
    include_remote: bool,
) -> Result<Vec<Branch>> {
    let git = git_for(&state, &repo_id)?;
    if !git.is_repo() {
        return Ok(Vec::new());
    }
    git.branches(include_remote)
}

#[tauri::command(async)]
pub fn checkout_branch(
    state: State<AppState>,
    repo_id: String,
    branch: String,
) -> Result<String> {
    git_for(&state, &repo_id)?.checkout(&branch)
}

#[tauri::command(async)]
pub fn create_branch(
    state: State<AppState>,
    repo_id: String,
    name: String,
    from: Option<String>,
    checkout: bool,
) -> Result<String> {
    let from = from.filter(|value| !value.trim().is_empty());
    git_for(&state, &repo_id)?.create_branch(&name, from.as_deref(), checkout)
}

#[tauri::command(async)]
pub fn delete_branch(
    state: State<AppState>,
    repo_id: String,
    branch: String,
    force: bool,
) -> Result<String> {
    let git = git_for(&state, &repo_id)?;
    if git.current_branch().unwrap_or_default() == branch {
        return Err(CoreError::git("不能删除当前所在分支，请先切换分支"));
    }
    git.delete_branch(&branch, force)
}

#[tauri::command(async)]
pub fn repo_log(state: State<AppState>, repo_id: String, limit: usize) -> Result<Vec<Commit>> {
    git_for(&state, &repo_id)?.log(if limit == 0 { 50 } else { limit })
}

#[tauri::command(async)]
pub fn commit_graph(
    state: State<AppState>,
    repo_id: String,
    head: Option<String>,
    limit: usize,
) -> Result<Vec<GraphCommit>> {
    let head = head.filter(|value| !value.trim().is_empty());
    git_for(&state, &repo_id)?.commit_graph(head.as_deref(), if limit == 0 { 200 } else { limit })
}

#[tauri::command(async)]
pub fn push_repo(state: State<AppState>, repo_id: String) -> Result<String> {
    git_for(&state, &repo_id)?.push()
}

#[tauri::command(async)]
pub fn repo_status(state: State<AppState>, repo_id: String) -> Result<RepoStatus> {
    git_for(&state, &repo_id)?.status()
}

#[tauri::command(async)]
pub fn file_diff(state: State<AppState>, repo_id: String, path: String) -> Result<String> {
    git_for(&state, &repo_id)?.diff_file(&path)
}

#[tauri::command(async)]
pub fn list_dir(state: State<AppState>, repo_id: String, path: String) -> Result<Vec<FileEntry>> {
    git_for(&state, &repo_id)?.list_dir(&path)
}

#[tauri::command(async)]
pub fn read_repo_file(
    state: State<AppState>,
    repo_id: String,
    path: String,
) -> Result<FileContent> {
    git_for(&state, &repo_id)?.read_file(&path)
}

#[tauri::command(async)]
pub fn write_repo_file(
    state: State<AppState>,
    repo_id: String,
    path: String,
    content: String,
) -> Result<String> {
    git_for(&state, &repo_id)?.write_file(&path, &content)
}

#[tauri::command(async)]
pub fn find_files(state: State<AppState>, repo_id: String, query: String) -> Result<Vec<String>> {
    git_for(&state, &repo_id)?.find_files(&query, 300)
}

#[tauri::command(async)]
pub fn search_content(
    state: State<AppState>,
    repo_id: String,
    query: String,
    case_sensitive: bool,
) -> Result<Vec<SearchHit>> {
    git_for(&state, &repo_id)?.search_content(&query, case_sensitive, 800)
}

#[tauri::command(async)]
pub fn replace_content(
    state: State<AppState>,
    repo_id: String,
    search: String,
    replacement: String,
    paths: Vec<String>,
    case_sensitive: bool,
) -> Result<ReplaceSummary> {
    git_for(&state, &repo_id)?.replace_content(&search, &replacement, &paths, case_sensitive)
}

#[tauri::command(async)]
pub fn commit_changes(
    state: State<AppState>,
    repo_id: String,
    message: String,
    allow_sensitive: bool,
) -> Result<String> {
    git_for(&state, &repo_id)?.commit_all(&message, allow_sensitive)
}

/// 列出本次改动中疑似包含敏感信息的文件（提交前确认用）。
#[tauri::command(async)]
pub fn sensitive_changes(state: State<AppState>, repo_id: String) -> Result<Vec<String>> {
    git_for(&state, &repo_id)?.sensitive_changes()
}

#[tauri::command(async)]
pub fn reset_hard(state: State<AppState>, repo_id: String, rev: String) -> Result<String> {
    git_for(&state, &repo_id)?.reset_hard(&rev)
}

#[tauri::command(async)]
pub fn fetch_repo(state: State<AppState>, repo_id: String) -> Result<String> {
    git_for(&state, &repo_id)?.fetch()
}

#[tauri::command(async)]
pub fn pull_repo(state: State<AppState>, repo_id: String) -> Result<String> {
    git_for(&state, &repo_id)?.pull()
}

#[tauri::command(async)]
pub fn resolve_rev(state: State<AppState>, repo_id: String, rev: String) -> Result<ResolvedRev> {
    git_for(&state, &repo_id)?.resolve(&rev)
}
