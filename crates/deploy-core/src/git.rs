use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::error::{CoreError, Result};
use crate::models::{
    Branch, Commit, FileChange, FileContent, FileEntry, GraphCommit, GraphRef, RepoStatus,
    ReplaceSummary, ResolvedRev, SearchHit,
};
use crate::process::{run, run_timeout, run_with_env, CommandOutput};

/// 记录分隔符（Unit Separator），避免提交信息中的普通字符造成解析歧义。
const SEP: char = '\u{1f}';

/// 网络类 Git 操作（fetch/pull/push）的超时时间。
const NETWORK_TIMEOUT_SECS: u64 = 600;

/// 对某个本地仓库执行 Git 操作。
pub struct Git {
    path: PathBuf,
}

impl Git {
    /// 打开一个本地目录。不要求目录已经是 Git 仓库，未初始化时可在绑定远端时自动执行 `git init`。
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let git = Self { path: path.into() };
        if !git.path.is_dir() {
            return Err(CoreError::not_found(format!(
                "目录不存在: {}",
                git.path.display()
            )));
        }
        Ok(git)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 目录是否位于 Git 仓库内。
    pub fn is_repo(&self) -> bool {
        self.try_run(&["rev-parse", "--is-inside-work-tree"])
            .map(|out| out.success() && out.stdout.trim() == "true")
            .unwrap_or(false)
    }

    fn base_args(&self, args: &[&str]) -> Vec<String> {
        let mut base = vec![
            "-C".to_string(),
            self.path.to_string_lossy().into_owned(),
            "-c".to_string(),
            "core.quotepath=false".to_string(),
        ];
        base.extend(args.iter().map(|arg| (*arg).to_string()));
        base
    }

    fn try_run(&self, args: &[&str]) -> Result<CommandOutput> {
        run("git", &self.base_args(args), None)
    }

    fn try_run_env(&self, args: &[&str], envs: &[(String, String)]) -> Result<CommandOutput> {
        run_with_env("git", &self.base_args(args), None, envs)
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let out = self.try_run(args)?;
        if !out.success() {
            return Err(CoreError::git(git_error(args, &out)));
        }
        Ok(out.stdout)
    }

    fn run_env(&self, args: &[&str], envs: &[(String, String)]) -> Result<String> {
        let out = self.try_run_env(args, envs)?;
        if !out.success() {
            return Err(CoreError::git(git_error(args, &out)));
        }
        Ok(out.stdout)
    }

    /// 需要访问网络的 Git 命令（fetch/pull/push），带超时避免永久卡死。
    fn run_network(&self, args: &[&str]) -> Result<CommandOutput> {
        run_timeout(
            "git",
            &self.base_args(args),
            None,
            Duration::from_secs(NETWORK_TIMEOUT_SECS),
        )
    }

    /// 获取当前分支名（分离头指针时为 HEAD；空仓库返回尚未提交的初始分支名）。
    pub fn current_branch(&self) -> Result<String> {
        // 空仓库没有 HEAD 提交，`rev-parse --abbrev-ref HEAD` 会直接失败；
        // `symbolic-ref` 对尚未提交的初始分支同样有效。
        let symbolic = self.try_run(&["symbolic-ref", "--short", "-q", "HEAD"])?;
        if symbolic.success() {
            let name = symbolic.stdout.trim();
            if !name.is_empty() {
                return Ok(name.to_string());
            }
        }
        Ok(self.run(&["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string())
    }

    /// 列出本地（可选远程）分支。
    pub fn branches(&self, include_remote: bool) -> Result<Vec<Branch>> {
        let mut refs: Vec<&str> = vec!["refs/heads"];
        if include_remote {
            refs.push("refs/remotes");
        }

        let format = format!(
            "%(refname){SEP}%(refname:short){SEP}%(objectname:short){SEP}%(upstream:short){SEP}\
             %(committerdate:format:%Y-%m-%d %H:%M){SEP}%(contents:subject)"
        );
        let mut args = vec!["for-each-ref"];
        args.extend(refs.iter().copied());
        args.push("--format");
        args.push(&format);
        args.push("--sort=-committerdate");

        let output = self.run(&args)?;
        let current = self.current_branch().unwrap_or_default();

        let mut branches = Vec::new();
        for line in output.lines() {
            let parts: Vec<&str> = line.split(SEP).collect();
            if parts.len() < 6 {
                continue;
            }
            let refname = parts[0];
            let short = parts[1];
            if refname.starts_with("refs/remotes/") && short.ends_with("/HEAD") {
                continue;
            }
            let is_remote = refname.starts_with("refs/remotes/");
            branches.push(Branch {
                name: short.to_string(),
                is_current: !is_remote && short == current,
                is_remote,
                upstream: non_empty(parts[3]),
                last_commit: parts[2].to_string(),
                last_commit_date: parts[4].to_string(),
                last_commit_subject: parts[5].to_string(),
            });
        }
        Ok(branches)
    }

    /// 工作区状态（当前分支、领先/落后提交数、变动文件）。
    pub fn status(&self) -> Result<RepoStatus> {
        let output = self.run(&["status", "--porcelain=v1", "-b", "--untracked-files=normal"])?;

        let mut status = RepoStatus {
            branch: String::new(),
            upstream: None,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
        };

        for line in output.lines() {
            if let Some(header) = line.strip_prefix("## ") {
                parse_status_header(header, &mut status);
                continue;
            }
            if line.len() < 3 {
                continue;
            }
            let (code, path) = line.split_at(2);
            status.changes.push(FileChange {
                code: code.to_string(),
                status: describe_code(code),
                path: path.trim().to_string(),
            });
        }
        Ok(status)
    }

    /// 提交记录。
    pub fn log(&self, limit: usize) -> Result<Vec<Commit>> {
        let limit_arg = format!("-n{limit}");
        let format = format!("%H{SEP}%h{SEP}%an{SEP}%ad{SEP}%s");
        let pretty_arg = format!("--pretty=format:{format}");
        let out = self.try_run(&[
            "log",
            &limit_arg,
            "--no-color",
            "--date=format:%Y-%m-%d %H:%M",
            &pretty_arg,
        ])?;
        if !out.success() {
            // 只有"空仓库"才静默返回空列表，其他失败如实报错，避免掩盖对象库损坏等问题。
            if is_empty_repo_error(&out) {
                return Ok(Vec::new());
            }
            return Err(CoreError::git(git_error(&["log"], &out)));
        }
        let mut commits = Vec::new();
        for line in out.stdout.lines() {
            let parts: Vec<&str> = line.split(SEP).collect();
            if parts.len() < 5 {
                continue;
            }
            commits.push(Commit {
                hash: parts[0].to_string(),
                short: parts[1].to_string(),
                author: parts[2].to_string(),
                date: parts[3].to_string(),
                subject: parts[4].to_string(),
            });
        }
        Ok(commits)
    }

    /// 工作区文件相对当前提交的差异（unified diff）。未跟踪文件按“新增”构造差异。
    pub fn diff_file(&self, path: &str) -> Result<String> {
        let rel = normalize_rel(path)?;
        if rel.is_empty() {
            return Err(CoreError::git("文件路径不能为空"));
        }
        // 空仓库（绑定后还没提交过）没有 HEAD，直接 `git diff HEAD` 会失败：
        // 已暂存文件用 `--cached` 展示，未暂存文件交给下方的"新增"差异逻辑。
        let text = if self.rev_hash("HEAD")?.is_some() {
            self.run(&["diff", "HEAD", "--", &rel])?.trim_end().to_string()
        } else {
            self.run(&["diff", "--cached", "--", &rel])
                .unwrap_or_default()
                .trim_end()
                .to_string()
        };
        if !text.is_empty() {
            return Ok(truncate_diff(text));
        }
        let status = self.run(&["status", "--porcelain", "--", &rel])?;
        if !status.lines().any(|line| line.starts_with("??")) {
            return Ok(String::new());
        }
        // 未跟踪文件可能是指向仓库外的符号链接：与 read_file 一致，解析真实路径并限制在仓库内。
        let file = self.path.join(&rel);
        let root =
            std::fs::canonicalize(&self.path).map_err(|e| CoreError::io_path(&self.path, e))?;
        let canonical = std::fs::canonicalize(&file)
            .map_err(|_| CoreError::git("二进制或无法读取的文件，暂不支持差异预览"))?;
        if !canonical.starts_with(&root) {
            return Err(CoreError::git(format!(
                "文件路径越界（可能是指向仓库外的符号链接）: {rel}"
            )));
        }
        // 与文件预览保持一致的大小上限，避免超大文件把差异内容全部读入内存。
        if std::fs::metadata(&file).map(|meta| meta.len()).unwrap_or(0) > 2_000_000 {
            return Err(CoreError::git("文件过大，暂不支持差异预览"));
        }
        let content = std::fs::read_to_string(&file)
            .map_err(|_| CoreError::git("二进制或无法读取的文件，暂不支持差异预览"))?;
        let added: Vec<String> = content.lines().map(|line| format!("+{line}")).collect();
        let mut diff = format!("diff --git a/{rel} b/{rel}\nnew file\n--- /dev/null\n+++ b/{rel}\n");
        diff.push_str(&added.join("\n"));
        if !content.is_empty() {
            diff.push('\n');
        }
        Ok(truncate_diff(diff))
    }

    /// 提交图数据：包含父提交与引用装饰，供前端渲染车道图。
    /// `ref_name` 为 Some 时只取该分支历史，否则取全部分支（--all）。
    pub fn commit_graph(&self, ref_name: Option<&str>, limit: usize) -> Result<Vec<GraphCommit>> {
        let limit_arg = format!("-n{limit}");
        let format = format!("%H{SEP}%h{SEP}%P{SEP}%an{SEP}%ad{SEP}%s{SEP}%D");
        let pretty_arg = format!("--pretty=format:{format}");
        let ref_name = match ref_name.map(str::trim).filter(|value| !value.is_empty()) {
            Some(name) => Some(validate_ref_arg(name, "分支")?),
            None => None,
        };
        let mut args: Vec<&str> = vec!["log"];
        match &ref_name {
            Some(name) => args.push(name),
            None => args.push("--all"),
        }
        args.extend([
            "--topo-order",
            &limit_arg,
            "--no-color",
            "--date=format:%Y-%m-%d %H:%M",
            &pretty_arg,
        ]);

        let out = self.try_run(&args)?;
        if !out.success() {
            if is_empty_repo_error(&out) {
                return Ok(Vec::new());
            }
            return Err(CoreError::git(git_error(&args, &out)));
        }

        let head_branch = self.current_branch().unwrap_or_default();
        let remotes = self.remote_names();

        let mut commits = Vec::new();
        for line in out.stdout.lines() {
            let parts: Vec<&str> = line.split(SEP).collect();
            if parts.len() < 7 {
                continue;
            }
            commits.push(GraphCommit {
                hash: parts[0].to_string(),
                short: parts[1].to_string(),
                parents: parts[2].split_whitespace().map(|p| p.to_string()).collect(),
                author: parts[3].to_string(),
                date: parts[4].to_string(),
                subject: parts[5].to_string(),
                refs: parse_decorations(parts[6], &head_branch, &remotes),
            });
        }
        Ok(commits)
    }

    /// 当前分支是否已设置上游。
    pub fn has_upstream(&self) -> bool {
        self.try_run(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
            .map(|out| out.success() && !out.stdout.trim().is_empty())
            .unwrap_or(false)
    }

    /// 推送当前分支（无上游时自动关联 origin 同名分支）。
    pub fn push(&self) -> Result<String> {
        // 空仓库还没有任何提交，git push 只会报 "src refspec ... does not match any"。
        if self.rev_hash("HEAD")?.is_none() {
            return Err(CoreError::git("还没有可推送的提交，请先提交变更"));
        }
        let branch = self.current_branch()?;
        let out = if self.has_upstream() {
            self.run_network(&["push"])?
        } else {
            self.run_network(&["push", "--set-upstream", "origin", &branch])?
        };
        if !out.success() {
            return Err(CoreError::git(git_error(&["push"], &out)));
        }
        let text = out.combined();
        Ok(if text.trim().is_empty() {
            format!("已推送 {branch}")
        } else {
            text.trim().to_string()
        })
    }

    /// 切换分支。
    pub fn checkout(&self, branch: &str) -> Result<String> {
        let branch = validate_ref_arg(branch, "分支")?;
        let out = self.try_run(&["checkout", &branch])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["checkout", &branch], &out)));
        }
        Ok(format!("已切换到分支 {branch}"))
    }

    /// 创建分支，可选基于某个起点并立即切换过去。
    pub fn create_branch(&self, name: &str, from: Option<&str>, checkout: bool) -> Result<String> {
        let name = validate_ref_arg(name, "分支")?;
        let from = match from.map(str::trim).filter(|value| !value.is_empty()) {
            Some(from) => Some(validate_ref_arg(from, "起点版本")?),
            None => None,
        };
        let command = if checkout { "checkout" } else { "branch" };
        let mut args = vec![command.to_string()];
        if checkout {
            args.push("-b".to_string());
        }
        args.push(name.clone());
        if let Some(from) = &from {
            args.push(from.clone());
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = self.try_run(&arg_refs)?;
        if !out.success() {
            return Err(CoreError::git(git_error(&arg_refs, &out)));
        }
        if checkout {
            Ok(format!("已创建并切换到分支 {name}"))
        } else {
            Ok(format!("已创建分支 {name}"))
        }
    }

    /// 删除分支（force = 强制删除未合并分支）。
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<String> {
        let name = validate_ref_arg(name, "分支")?;
        let force_arg = if force { "-D" } else { "-d" };
        let out = self.try_run(&["branch", force_arg, &name])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["branch", force_arg, &name], &out)));
        }
        Ok(format!("已删除分支 {name}"))
    }

    /// 拉取远端更新（含清理已删除的远端分支）。
    pub fn fetch(&self) -> Result<String> {
        let out = self.run_network(&["fetch", "--all", "--prune"])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["fetch", "--all", "--prune"], &out)));
        }
        let text = out.combined();
        if text.trim().is_empty() {
            Ok("已是最新状态".to_string())
        } else {
            Ok(text.trim().to_string())
        }
    }

    /// 拉取并合并当前分支的上游。
    pub fn pull(&self) -> Result<String> {
        let out = self.run_network(&["pull"])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["pull"], &out)));
        }
        let text = out.combined();
        Ok(if text.trim().is_empty() {
            "已是最新状态".to_string()
        } else {
            text.trim().to_string()
        })
    }

    /// 提交全部改动；`allow_sensitive` 为 false 时拒绝提交疑似敏感文件。
    pub fn commit_all(&self, message: &str, allow_sensitive: bool) -> Result<String> {
        let message = message.trim();
        if message.is_empty() {
            return Err(CoreError::git("提交信息不能为空"));
        }
        let status = self.run(&["status", "--porcelain"])?;
        if status.trim().is_empty() {
            return Err(CoreError::git("没有需要提交的改动"));
        }
        if !allow_sensitive {
            let sensitive = self.sensitive_changes()?;
            if !sensitive.is_empty() {
                return Err(CoreError::git(format!(
                    "检测到疑似敏感文件，已阻止提交：\n  {}\n如确认不含密钥 / 密码：界面中可再次确认提交，命令行需加 --allow-sensitive。",
                    sensitive.join("\n  ")
                )));
            }
        }
        self.run(&["add", "-A"])?;
        let out = self.try_run(&["commit", "-m", message])?;
        if !out.success() {
            return Err(CoreError::git(git_error(&["commit", "-m", message], &out)));
        }
        let short = self.run(&["rev-parse", "--short", "HEAD"])?;
        Ok(format!("已提交 {}: {}", short.trim(), message))
    }

    /// 列出本次改动中疑似包含敏感信息的文件（按文件名判断，含未跟踪文件）。
    pub fn sensitive_changes(&self) -> Result<Vec<String>> {
        // -z 输出以 NUL 分隔且不做引号转义：重命名会额外输出旧路径段，统一收集判断即可，
        // 避免 ` -> ` 出现在文件名里或路径被 git 引号包裹时漏检。
        let out = self.run(&["status", "--porcelain", "-z", "--untracked-files=all"])?;
        let mut files: Vec<String> = Vec::new();
        for entry in out.split('\0') {
            if entry.is_empty() {
                continue;
            }
            // 常规条目形如 "XY path"（前两列状态 + 一个空格），否则是重命名的旧路径段。
            let path = if entry.len() > 3 && entry.as_bytes()[2] == b' ' {
                &entry[3..]
            } else {
                entry
            }
            .replace('\\', "/");
            if is_sensitive_path(&path) && !files.contains(&path) {
                files.push(path);
            }
        }
        Ok(files)
    }

    /// 回退到指定版本（丢弃工作区改动）。
    pub fn reset_hard(&self, rev: &str) -> Result<String> {
        let resolved = self.resolve(rev)?;
        let out = self.try_run(&["reset", "--hard", &resolved.hash])?;
        if !out.success() {
            return Err(CoreError::git(git_error(
                &["reset", "--hard", &resolved.hash],
                &out,
            )));
        }
        Ok(format!(
            "已回退到 {} {}",
            resolved.short, resolved.subject
        ))
    }

    /// 将分支名 / 标签 / 提交号解析为具体提交。
    pub fn resolve(&self, rev: &str) -> Result<ResolvedRev> {
        let rev = validate_ref_arg(rev, "版本号")?;
        let hash = match self.rev_hash(&rev)? {
            Some(hash) => hash,
            None => {
                let remote_rev = format!("origin/{rev}");
                self.rev_hash(&remote_rev)?
                    .ok_or_else(|| CoreError::git(format!("找不到版本: {rev}")))? 
                    .to_string()
            }
        };

        let format = format!("%h{SEP}%s{SEP}%an{SEP}%ad");
        let pretty_arg = format!("--pretty=format:{format}");
        let out = self.run(&[
            "log",
            "-1",
            "--no-color",
            "--date=format:%Y-%m-%d %H:%M",
            &pretty_arg,
            &hash,
        ])?;
        let parts: Vec<&str> = out.trim().split(SEP).collect();
        Ok(ResolvedRev {
            rev: rev.to_string(),
            hash,
            short: parts.first().copied().unwrap_or_default().to_string(),
            subject: parts.get(1).copied().unwrap_or_default().to_string(),
            author: parts.get(2).copied().unwrap_or_default().to_string(),
            date: parts.get(3).copied().unwrap_or_default().to_string(),
        })
    }

    fn rev_hash(&self, rev: &str) -> Result<Option<String>> {
        let expr = format!("{rev}^{{commit}}");
        let out = self.try_run(&["rev-parse", "--verify", "--quiet", &expr])?;
        if out.success() {
            let hash = out.stdout.trim().to_string();
            if hash.is_empty() {
                Ok(None)
            } else {
                Ok(Some(hash))
            }
        } else {
            Ok(None)
        }
    }

    /// 将指定提交打包为 tar，并压缩为 tar.gz。
    ///
    /// 使用 `git archive` + Rust 端 gzip，避免依赖本机 gzip 程序。
    /// 返回压缩包大小（字节）。
    pub fn archive(&self, rev: &str, tar_path: &Path, gz_path: &Path) -> Result<u64> {
        let tar_str = tar_path.to_string_lossy().into_owned();
        let output = self.try_run(&["archive", "--format=tar", "-o", &tar_str, rev])?;
        if !output.success() {
            return Err(CoreError::git(git_error(
                &["archive", "--format=tar", "-o", &tar_str, rev],
                &output,
            )));
        }
        compress_tar(tar_path, gz_path)
    }

    /// 将当前工作区（含未提交改动与未跟踪文件）打包为 tar.gz，返回压缩包大小（字节）。
    ///
    /// 借助临时索引执行 `git add -A` + `git write-tree` 生成工作区快照，再交给
    /// `git archive` 打包：不会改动真实暂存区，未跟踪文件遵循 `.gitignore` 规则。
    pub fn archive_worktree(&self, tar_path: &Path, gz_path: &Path) -> Result<u64> {
        let index_path = tar_path.with_extension("worktree-index");
        let _ = std::fs::remove_file(&index_path);
        let envs = vec![(
            "GIT_INDEX_FILE".to_string(),
            index_path.to_string_lossy().into_owned(),
        )];
        let result = (|| -> Result<u64> {
            let add = self.try_run_env(&["add", "-A"], &envs)?;
            if !add.success() {
                return Err(CoreError::git(git_error(&["add", "-A"], &add)));
            }
            let tree = self.run_env(&["write-tree"], &envs)?;
            let tree = tree.trim().to_string();
            let tar_str = tar_path.to_string_lossy().into_owned();
            let output = self.try_run(&["archive", "--format=tar", "-o", &tar_str, &tree])?;
            if !output.success() {
                return Err(CoreError::git(git_error(
                    &["archive", "--format=tar", "-o", &tar_str, &tree],
                    &output,
                )));
            }
            compress_tar(tar_path, gz_path)
        })();
        // 临时索引只是打包用的中间产物，成功失败都要清理。
        let _ = std::fs::remove_file(&index_path);
        result
    }

    /// 远端仓库地址。
    pub fn remote_url(&self) -> Result<Option<String>> {
        let out = self.try_run(&["remote", "get-url", "origin"])?;
        if out.success() {
            Ok(non_empty(out.stdout.trim()))
        } else {
            Ok(None)
        }
    }

    /// 任意可用的远端地址：优先 origin，其次其他 remote（如 upstream）。
    pub fn any_remote_url(&self) -> Result<Option<String>> {
        if let Some(url) = self.remote_url()? {
            return Ok(Some(url));
        }
        for name in self.remote_names() {
            let out = self.try_run(&["remote", "get-url", &name])?;
            if out.success() {
                if let Some(url) = non_empty(out.stdout.trim()) {
                    return Ok(Some(url));
                }
            }
        }
        Ok(None)
    }

    /// 克隆远端仓库到指定目录（目标目录必须不存在或为空）。
    pub fn clone(remote: &str, dest: &Path) -> Result<()> {
        let remote = validate_remote_url(remote)?;
        let args = vec![
            "clone".to_string(),
            "--progress".to_string(),
            remote,
            dest.to_string_lossy().into_owned(),
        ];
        let out = crate::process::run_timeout("git", &args, None, Duration::from_secs(900))
            .map_err(|e| CoreError::git(format!("克隆失败: {e}")))?;
        if !out.success() {
            return Err(CoreError::git(format!("克隆失败: {}", out.combined().trim())));
        }
        Ok(())
    }

    /// 绑定或更新远端地址（origin）；目录尚未初始化 Git 时自动执行 `git init`。
    pub fn set_remote_url(&self, url: &str) -> Result<()> {
        let url = validate_remote_url(url)?;
        if !self.is_repo() {
            let out = self.try_run(&["init"])?;
            if !out.success() {
                return Err(CoreError::git(git_error(&["init"], &out)));
            }
        }
        if self.remote_names().iter().any(|name| name == "origin") {
            self.run(&["remote", "set-url", "origin", &url])?;
        } else {
            self.run(&["remote", "add", "origin", &url])?;
        }
        Ok(())
    }

    fn remote_names(&self) -> Vec<String> {
        self.try_run(&["remote"])
            .map(|out| {
                out.stdout
                    .lines()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 列出仓库内某个目录（相对路径，空串表示根目录）的内容，目录在前、按名称排序。
    pub fn list_dir(&self, rel: &str) -> Result<Vec<FileEntry>> {
        let rel = normalize_rel(rel)?;
        let dir = self.path.join(&rel);
        if !dir.is_dir() {
            return Err(CoreError::not_found(format!("目录不存在: {rel}")));
        }
        let read = std::fs::read_dir(&dir).map_err(|e| CoreError::io_path(&dir, e))?;
        let mut entries = Vec::new();
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if rel.is_empty() && name == ".git" {
                continue;
            }
            let is_dir = item.path().is_dir();
            let size = if is_dir {
                0
            } else {
                item.metadata().map(|meta| meta.len()).unwrap_or(0)
            };
            let path = if rel.is_empty() {
                name.clone()
            } else {
                format!("{rel}/{name}")
            };
            entries.push(FileEntry {
                name,
                path,
                is_dir,
                size,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// 读取仓库内文本文件用于预览，超大文件截断，二进制文件返回错误。
    pub fn read_file(&self, rel: &str) -> Result<FileContent> {
        let rel = normalize_rel(rel)?;
        if rel.is_empty() {
            return Err(CoreError::git("文件路径不能为空"));
        }
        const LIMIT: usize = 2_000_000;
        let file = self.path.join(&rel);
        if !file.is_file() {
            return Err(CoreError::not_found(format!("文件不存在: {rel}")));
        }
        // 仓库里可能存在指向仓库外的符号链接，解析真实路径后必须仍在仓库内。
        let root =
            std::fs::canonicalize(&self.path).map_err(|e| CoreError::io_path(&self.path, e))?;
        let canonical = std::fs::canonicalize(&file).map_err(|e| CoreError::io_path(&file, e))?;
        if !canonical.starts_with(&root) {
            return Err(CoreError::git(format!(
                "文件路径越界（可能是指向仓库外的符号链接）: {rel}"
            )));
        }
        let bytes = std::fs::read(&file).map_err(|e| CoreError::io_path(&file, e))?;
        if bytes[..bytes.len().min(8000)].contains(&0) {
            return Err(CoreError::git("二进制文件，暂不支持预览"));
        }
        let truncated = bytes.len() > LIMIT;
        let text = String::from_utf8_lossy(&bytes[..bytes.len().min(LIMIT)]).into_owned();
        Ok(FileContent {
            path: rel,
            content: text,
            truncated,
        })
    }

    /// 保存文本内容到仓库内的文件（简单编辑器写回）。
    pub fn write_file(&self, rel: &str, content: &str) -> Result<String> {
        let rel = normalize_rel(rel)?;
        if rel.is_empty() {
            return Err(CoreError::git("文件路径不能为空"));
        }
        let file = self.path.join(&rel);
        if file.is_dir() {
            return Err(CoreError::git(format!("目标是目录: {rel}")));
        }
        let root =
            std::fs::canonicalize(&self.path).map_err(|e| CoreError::io_path(&self.path, e))?;
        // 已存在的文件解析符号链接后必须仍在仓库内；新文件则要求其父目录
        // （解析符号链接后）位于仓库内，避免经由链接目录写到仓库外。
        match std::fs::symlink_metadata(&file) {
            Ok(_) => {
                let canonical =
                    std::fs::canonicalize(&file).map_err(|e| CoreError::io_path(&file, e))?;
                if !canonical.starts_with(&root) {
                    return Err(CoreError::git(format!(
                        "文件路径越界（可能是指向仓库外的符号链接）: {rel}"
                    )));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let parent = file
                    .parent()
                    .ok_or_else(|| CoreError::git(format!("非法路径: {rel}")))?;
                let canonical_parent =
                    std::fs::canonicalize(parent).map_err(|e| CoreError::io_path(parent, e))?;
                if !canonical_parent.starts_with(&root) {
                    return Err(CoreError::git(format!(
                        "文件路径越界（父目录指向仓库外）: {rel}"
                    )));
                }
            }
            Err(err) => return Err(CoreError::io_path(&file, err)),
        }
        // 普通文件走「临时文件 + rename」原子写，避免写入中途失败截断原文件；
        // 指向仓库内其他文件的符号链接保持写穿语义，不做替换。
        let is_symlink = std::fs::symlink_metadata(&file)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink {
            std::fs::write(&file, content.as_bytes()).map_err(|e| CoreError::io_path(&file, e))?;
        } else {
            atomic_write_text(&file, content)?;
        }
        Ok(format!("已保存 {rel}"))
    }

    /// 按文件名快速搜索（tracked + 未忽略的 untracked），大小写不敏感子串匹配。
    /// 非 Git 文件夹退化为遍历本地目录。
    pub fn find_files(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        if !self.is_repo() {
            let needle = query.trim().to_lowercase();
            let mut files = Vec::new();
            walk_files(&self.path, |rel, _| {
                if needle.is_empty() || rel.to_lowercase().contains(&needle) {
                    files.push(rel.to_string());
                    files.len() >= limit
                } else {
                    false
                }
            });
            files.sort();
            return Ok(files);
        }
        let out = self.try_run(&["ls-files", "--cached", "--others", "--exclude-standard"])?;
        if !out.success() {
            return Ok(Vec::new());
        }
        let needle = query.trim().to_lowercase();
        let mut files = Vec::new();
        for line in out.stdout.lines() {
            let path = line.trim();
            if path.is_empty() {
                continue;
            }
            if needle.is_empty() || path.to_lowercase().contains(&needle) {
                files.push(path.to_string());
                if files.len() >= limit {
                    break;
                }
            }
        }
        Ok(files)
    }

    /// 全仓库内容搜索（git grep 固定字符串，含未跟踪文件，跳过二进制）。
    /// 非 Git 文件夹退化为遍历本地目录逐行匹配。
    pub fn search_content(
        &self,
        query: &str,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        if !self.is_repo() {
            return self.search_content_fs(query, case_sensitive, limit);
        }
        // git grep 的 `-<n>` 是上下文行数（-C <n>）而非条数上限，必须用 --max-count；
        // 且 --max-count 是「每个文件」上限，解析时再按总量截断。
        let max_arg = format!("--max-count={limit}");
        let mut args: Vec<&str> = vec!["grep", "-n", "-I", "--untracked", &max_arg];
        if !case_sensitive {
            args.push("-i");
        }
        // 用 -e 显式标记 pattern，避免以 `-` 开头的搜索词被 git 当作选项。
        args.extend(["--fixed-strings", "-e", query]);
        let out = self.try_run(&args)?;
        if out.code != 0 && out.code != 1 {
            return Err(CoreError::git(git_error(&args, &out)));
        }
        let mut hits = Vec::new();
        for line in out.stdout.lines() {
            let mut parts = line.splitn(3, ':');
            let (Some(path), Some(num), Some(text)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(num) = num.parse::<u32>() else {
                continue;
            };
            hits.push(SearchHit {
                path: path.to_string(),
                line: num,
                text: text.trim_start().to_string(),
            });
            if hits.len() >= limit {
                break;
            }
        }
        Ok(hits)
    }

    /// 非 Git 文件夹的全文搜索：遍历目录逐行匹配，跳过二进制与超大文件。
    fn search_content_fs(
        &self,
        query: &str,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        /// 单个文件最多读取的字节数，与文件预览的上限保持一致。
        const MAX_FILE_SIZE: u64 = 2_000_000;
        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        let mut hits = Vec::new();
        walk_files(&self.path, |rel, path| {
            let Ok(meta) = std::fs::metadata(path) else {
                return false;
            };
            if !meta.is_file() || meta.len() > MAX_FILE_SIZE {
                return false;
            }
            let Ok(bytes) = std::fs::read(path) else {
                return false;
            };
            if bytes[..bytes.len().min(8000)].contains(&0) {
                return false;
            }
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                let matched = if case_sensitive {
                    line.contains(&needle)
                } else {
                    line.to_lowercase().contains(&needle)
                };
                if !matched {
                    continue;
                }
                hits.push(SearchHit {
                    path: rel.to_string(),
                    line: (index + 1) as u32,
                    text: line.trim_start().to_string(),
                });
                if hits.len() >= limit {
                    return true;
                }
            }
            false
        });
        Ok(hits)
    }

    /// 在指定文件列表内做字面量批量替换，跳过二进制/非 UTF-8 文件。
    pub fn replace_content(
        &self,
        search: &str,
        replacement: &str,
        paths: &[String],
        case_sensitive: bool,
    ) -> Result<ReplaceSummary> {
        if search.is_empty() {
            return Err(CoreError::git("搜索内容不能为空"));
        }
        let mut files = 0u32;
        let mut matches = 0u32;
        let root =
            std::fs::canonicalize(&self.path).map_err(|e| CoreError::io_path(&self.path, e))?;
        for rel in paths {
            let Ok(rel) = normalize_rel(rel) else {
                continue;
            };
            if rel.is_empty() {
                continue;
            }
            let file = self.path.join(&rel);
            if !file.is_file() {
                continue;
            }
            // 与 write_file 一致：解析符号链接后必须仍在仓库内，避免经链接目录写到仓库外。
            let Ok(canonical) = std::fs::canonicalize(&file) else {
                continue;
            };
            if !canonical.starts_with(&root) {
                continue;
            }
            let bytes = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let (next, count) = replace_all_occurrences(&text, search, replacement, case_sensitive);
            if count > 0 {
                atomic_write_text(&file, &next)?;
                files += 1;
                matches += count;
            }
        }
        Ok(ReplaceSummary {
            files_replaced: files,
            matches_replaced: matches,
        })
    }
}

/// 将 tar 压缩为 tar.gz，返回压缩包大小（字节），并删除原始 tar。
fn compress_tar(tar_path: &Path, gz_path: &Path) -> Result<u64> {
    let src = File::open(tar_path).map_err(|e| CoreError::io_path(tar_path, e))?;
    let dst = File::create(gz_path).map_err(|e| CoreError::io_path(gz_path, e))?;
    let mut encoder = GzEncoder::new(BufWriter::new(dst), Compression::default());
    let mut reader = std::io::BufReader::new(src);
    std::io::copy(&mut reader, &mut encoder)?;
    encoder.flush()?;
    // finish() 只把 gzip 尾部写进 BufWriter，必须显式 flush/sync，否则最终写盘错误会被 drop 静默吞掉。
    let mut writer = encoder.finish()?;
    writer.flush().map_err(|e| CoreError::io_path(gz_path, e))?;
    writer
        .into_inner()
        .map_err(|e| CoreError::io_path(gz_path, e.into_error()))?
        .sync_all()
        .map_err(|e| CoreError::io_path(gz_path, e))?;
    let _ = std::fs::remove_file(tar_path);
    // 返回实际压缩包大小（io::copy 返回的是未压缩的 tar 字节数）。
    let size = std::fs::metadata(gz_path)
        .map_err(|e| CoreError::io_path(gz_path, e))?
        .len();
    Ok(size)
}

/// 非 Git 文件夹的文件遍历：跳过 `.git`、`node_modules` 与符号链接（避免环路），
/// 回调收到相对路径与绝对路径；回调返回 true 表示停止遍历。
fn walk_files(root: &Path, mut visit: impl FnMut(&str, &Path) -> bool) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in read.flatten() {
            let Ok(file_type) = item.file_type() else {
                continue;
            };
            let name = item.file_name().to_string_lossy().into_owned();
            if name == ".git" || name == "node_modules" || file_type.is_symlink() {
                continue;
            }
            let path = item.path();
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if rel.is_empty() {
                continue;
            }
            if visit(&rel, &path) {
                return;
            }
        }
    }
}

/// 字面量替换全部出现位置，返回新文本与替换次数。
fn replace_all_occurrences(
    text: &str,
    search: &str,
    replacement: &str,
    case_sensitive: bool,
) -> (String, u32) {
    if search.is_empty() {
        return (text.to_string(), 0);
    }
    if case_sensitive {
        let count = text.matches(search).count() as u32;
        return (text.replace(search, replacement), count);
    }

    // 将原文按“小写展开”映射为字符序列，并记录每个小写字符对应原文的字节区间。
    // to_lowercase 可能改变字节长度（如 İ、ẞ），直接复用字节偏移会越界 panic。
    struct LowerUnit {
        byte_start: usize,
        byte_end: usize,
        ch: char,
    }

    let mut units: Vec<LowerUnit> = Vec::with_capacity(text.len());
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        for lower in ch.to_lowercase() {
            units.push(LowerUnit {
                byte_start: start,
                byte_end: end,
                ch: lower,
            });
        }
    }

    let needle: Vec<char> = search.to_lowercase().chars().collect();
    if needle.is_empty() {
        return (text.to_string(), 0);
    }

    let mut result = String::with_capacity(text.len());
    let mut written = 0usize;
    let mut index = 0usize;
    let mut count = 0u32;
    while index + needle.len() <= units.len() {
        let matched = units[index..index + needle.len()]
            .iter()
            .map(|unit| unit.ch)
            .eq(needle.iter().copied());
        if matched {
            let start = units[index].byte_start;
            let end = units[index + needle.len() - 1].byte_end;
            // 匹配起点落在已写入字符内部（如 İ 展开为两个 lowercase 单元）时跳过，
            // 否则 `text[written..start]` 会因 start < written 而 panic。
            if start < written {
                index += 1;
                continue;
            }
            result.push_str(&text[written..start]);
            result.push_str(replacement);
            count += 1;
            index += needle.len();
            written = end;
        } else {
            index += 1;
        }
    }
    result.push_str(&text[written..]);
    (result, count)
}

/// 规范化仓库内相对路径，阻止越界访问。
fn normalize_rel(rel: &str) -> Result<String> {
    let rel = rel.trim().replace('\\', "/");
    if rel.is_empty() || rel == "." {
        return Ok(String::new());
    }
    if rel.starts_with('/') || rel.contains(":") || rel.split('/').any(|seg| seg == "..") {
        return Err(CoreError::git(format!("非法路径: {rel}")));
    }
    Ok(rel.trim_matches('/').to_string())
}

/// 校验用户提供的分支 / 版本 / 引用参数，避免以 `-` 开头被 git 当作选项
/// （例如 `git checkout -f` 会丢弃工作区全部未提交改动）。
fn validate_ref_arg(value: &str, label: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CoreError::git(format!("{label}不能为空")));
    }
    if value.starts_with('-') {
        return Err(CoreError::git(format!("非法的{label}: {value}")));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(CoreError::git(format!("{label}包含非法字符")));
    }
    Ok(value.to_string())
}

/// 校验用户填写的远端地址，避免以 `-` 开头被 git 当作选项。
fn validate_remote_url(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CoreError::config("Git 地址不能为空"));
    }
    if value.starts_with('-') {
        return Err(CoreError::config(format!("非法的 Git 地址: {value}")));
    }
    if value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(CoreError::config("Git 地址不能包含空白或控制字符"));
    }
    Ok(value.to_string())
}

/// 判断 git 失败输出是否表示"仓库还没有任何提交"。
fn is_empty_repo_error(out: &CommandOutput) -> bool {
    let text = out.combined().to_ascii_lowercase();
    text.contains("does not have any commits")
        || text.contains("bad revision 'head'")
        || text.contains("unknown revision or path not in the working tree")
}

/// 原子写回替换后的文本：同目录临时文件 + 刷盘 + rename，避免中途失败截断源文件。
fn atomic_write_text(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let tmp = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

    let write = (|| -> Result<()> {
        let mut file = File::create(&tmp).map_err(|e| CoreError::io_path(&tmp, e))?;
        file.write_all(contents.as_bytes())
            .map_err(|e| CoreError::io_path(&tmp, e))?;
        file.sync_all().map_err(|e| CoreError::io_path(&tmp, e))?;
        Ok(())
    })();
    if let Err(err) = write {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }

    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(CoreError::io_path(path, err));
    }
    Ok(())
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// 截断过大的差异文本，按 char 边界安全切割。
fn truncate_diff(mut text: String) -> String {
    const LIMIT: usize = 400_000;
    if text.len() > LIMIT {
        let mut cut = LIMIT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… 差异内容过大，已截断");
    }
    text
}

/// 解析 git log %D 装饰（如 `HEAD -> main, origin/main, tag: v1.0`）。
fn parse_decorations(decor: &str, head_branch: &str, remotes: &[String]) -> Vec<GraphRef> {
    let mut refs = Vec::new();
    for part in decor.split(", ") {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(tag) = part.strip_prefix("tag: ") {
            refs.push(GraphRef {
                name: tag.to_string(),
                kind: "tag".to_string(),
                is_head: false,
            });
            continue;
        }
        let (is_head, name) = match part.strip_prefix("HEAD -> ") {
            Some(name) => (true, name),
            None => {
                if part == "HEAD" {
                    continue;
                }
                (part == head_branch, part)
            }
        };
        let prefix = name.split('/').next().unwrap_or("");
        let kind = if remotes.iter().any(|remote| remote == prefix) {
            "remote"
        } else {
            "local"
        };
        refs.push(GraphRef {
            name: name.to_string(),
            kind: kind.to_string(),
            is_head,
        });
    }
    refs
}

fn parse_status_header(header: &str, status: &mut RepoStatus) {
    if let Some((left, right)) = header.split_once("...") {
        status.branch = left.trim().to_string();

        let mut upstream_part = right;
        if let Some(start) = right.find('[') {
            upstream_part = &right[..start];
            // 从 '[' 之后查找 ']'，否则分支名中先出现的 ']' 会导致切片越界 panic。
            if let Some(offset) = right[start..].find(']') {
                let end = start + offset;
                let flags = &right[start + 1..end];
                for flag in flags.split(',') {
                    let flag = flag.trim();
                    if let Some(value) = flag.strip_prefix("ahead ") {
                        status.ahead = value.trim().parse().unwrap_or(0);
                    } else if let Some(value) = flag.strip_prefix("behind ") {
                        status.behind = value.trim().parse().unwrap_or(0);
                    }
                }
            }
        }
        status.upstream = non_empty(upstream_part);
    } else if let Some(branch) = header.strip_prefix("No commits yet on ") {
        status.branch = branch.trim().to_string();
    } else if header.starts_with("HEAD") {
        status.branch = "HEAD".to_string();
    } else {
        status.branch = header.trim().to_string();
    }
}

fn describe_code(code: &str) -> String {
    let trimmed = code.trim();
    match trimmed {
        "??" => "未跟踪".to_string(),
        _ if trimmed.contains('U') => "冲突".to_string(),
        _ if trimmed.contains('R') => "重命名".to_string(),
        _ if trimmed.contains('C') => "复制".to_string(),
        _ if trimmed.contains('D') => "删除".to_string(),
        _ if trimmed.contains('A') => "新增".to_string(),
        _ if trimmed.contains('M') => "修改".to_string(),
        _ => code.trim().to_string(),
    }
}

/// 按文件名判断是否疑似敏感文件（模板 / 示例 / 公钥文件除外）。
fn is_sensitive_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    // 公钥与示例文件不敏感，避免误报阻断提交。
    if name.ends_with(".pub")
        || [".example", ".sample", ".template", ".dist"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
    {
        return false;
    }
    name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".env")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".pfx")
        || name.ends_with(".p12")
        || name.ends_with(".keystore")
        || name.ends_with(".jks")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
        || name.starts_with("id_ecdsa")
        || name.starts_with("id_dsa")
        || name == ".npmrc"
        || name == ".pypirc"
        || name == ".netrc"
        || name == ".git-credentials"
        || name.contains("credentials")
        || name.starts_with("secrets")
        || name.ends_with(".secret")
}

fn git_error(args: &[&str], out: &CommandOutput) -> String {
    let detail = out.combined();
    let detail = if detail.is_empty() { "无输出" } else { &detail };
    if detail.contains("Please tell me who you are") {
        return "Git 未配置提交身份，请先执行:\n  git config --global user.name \"你的名字\"\n  git config --global user.email \"你的邮箱\"".to_string();
    }
    if detail.contains("not a git repository") {
        return "当前文件夹尚未绑定 Git 仓库，请先绑定远端仓库地址".to_string();
    }
    format!("git {} 失败: {detail}", args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_status() -> RepoStatus {
        RepoStatus {
            branch: String::new(),
            upstream: None,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
        }
    }

    #[test]
    fn parse_status_header_handles_bracket_in_branch_name() {
        // 分支名含 ']' 时旧实现会因 `right.find(']')` 从字符串开头查找而切片越界 panic。
        let mut status = empty_status();
        parse_status_header("foo]bar...origin/foo]bar [ahead 1, behind 2]", &mut status);
        assert_eq!(status.branch, "foo]bar");
        assert_eq!(status.upstream.as_deref(), Some("origin/foo]bar"));
        assert_eq!(status.ahead, 1);
        assert_eq!(status.behind, 2);
    }

    #[test]
    fn parse_status_header_parses_counts() {
        let mut status = empty_status();
        parse_status_header("main...origin/main [ahead 3]", &mut status);
        assert_eq!(status.branch, "main");
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.ahead, 3);
        assert_eq!(status.behind, 0);
    }

    #[test]
    fn replace_case_insensitive_basic() {
        let (text, count) = replace_all_occurrences("Hello World hello", "hello", "hi", false);
        assert_eq!(count, 2);
        assert_eq!(text, "hi World hi");
    }

    #[test]
    fn replace_case_insensitive_handles_unicode_length_change() {
        // İ 小写后字节数变化，旧实现复用 lower_text 的偏移索引原文会 panic。
        let (text, count) = replace_all_occurrences("İx", "x", "y", false);
        assert_eq!(count, 1);
        assert_eq!(text, "İy");
    }

    #[test]
    fn replace_empty_search_is_noop() {
        let (text, count) = replace_all_occurrences("abc", "", "x", false);
        assert_eq!(count, 0);
        assert_eq!(text, "abc");
    }

    #[test]
    fn replace_case_insensitive_skips_match_inside_expanded_char() {
        // İ 展开为 i + U+0307 两个 lower 单元（共享同一字节区间）：
        // 第二次匹配起点落在第一次匹配已写入的字符内部时，旧实现会
        // `&text[written..start]` 越界 panic。
        let input = "\u{0307}x\u{0130}xi";
        let (text, count) = replace_all_occurrences(input, "\u{0307}xi", "R", false);
        assert_eq!(count, 1);
        assert_eq!(text, "Rxi");
    }

    #[test]
    fn validate_remote_url_rejects_option_like_and_blank() {
        assert!(validate_remote_url("git@github.com:user/repo.git").is_ok());
        assert!(validate_remote_url("https://github.com/user/repo.git").is_ok());
        assert!(validate_remote_url("  ").is_err());
        assert!(validate_remote_url("--upload-pack=evil").is_err());
        assert!(validate_remote_url("https://x y/z").is_err());
    }

    #[test]
    fn validate_ref_arg_rejects_option_like_names() {
        assert!(validate_ref_arg("main", "分支").is_ok());
        assert!(validate_ref_arg("origin/main", "分支").is_ok());
        assert!(validate_ref_arg("-f", "分支").is_err());
        assert!(validate_ref_arg("--force", "分支").is_err());
        assert!(validate_ref_arg("  ", "分支").is_err());
        assert!(validate_ref_arg("a\nb", "分支").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn file_ops_reject_symlink_escape() {
        use std::os::unix::fs::symlink;

        let base =
            std::env::temp_dir().join(format!("deploycode-git-escape-{}", uuid::Uuid::new_v4()));
        let repo = base.join("repo");
        let outside = base.join("outside");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "top secret").unwrap();

        let git = Git { path: repo.clone() };

        // 指向仓库外文件的符号链接：读写都必须被拒绝。
        symlink(outside.join("secret.txt"), repo.join("link.txt")).unwrap();
        assert!(git.read_file("link.txt").is_err());
        assert!(git.write_file("link.txt", "hacked").is_err());
        assert_eq!(
            std::fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "top secret"
        );

        // 指向仓库外目录的符号链接：经由它读写同样被拒绝。
        symlink(&outside, repo.join("dirlink")).unwrap();
        assert!(git.read_file("dirlink/secret.txt").is_err());
        assert!(git.write_file("dirlink/new.txt", "x").is_err());
        assert!(!outside.join("new.txt").exists());

        // 仓库内的普通文件读写不受影响。
        std::fs::write(repo.join("ok.txt"), "hi").unwrap();
        assert_eq!(git.read_file("ok.txt").unwrap().content, "hi");
        assert!(git.write_file("ok.txt", "hello").is_ok());
        assert_eq!(
            std::fs::read_to_string(repo.join("ok.txt")).unwrap(),
            "hello"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[test]
    fn open_missing_dir_errors() {
        let missing =
            std::env::temp_dir().join(format!("deploycode-git-missing-{}", uuid::Uuid::new_v4()));
        assert!(Git::open(&missing).is_err());
    }

    #[test]
    fn open_non_git_dir_is_allowed() {
        let dir =
            std::env::temp_dir().join(format!("deploycode-git-plain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let git = Git::open(&dir).unwrap();
        assert!(!git.is_repo());
        if git_available() {
            assert_eq!(git.remote_url().unwrap(), None);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binding_remote_initializes_non_git_dir() {
        if !git_available() {
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("deploycode-git-init-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let git = Git::open(&dir).unwrap();
        git.set_remote_url("https://github.com/user/repo.git").unwrap();
        assert!(git.is_repo());
        assert_eq!(
            git.remote_url().unwrap().as_deref(),
            Some("https://github.com/user/repo.git")
        );

        // 已初始化后再次绑定只更新地址，不重复 init。
        git.set_remote_url("git@github.com:user/other.git").unwrap();
        assert_eq!(
            git.remote_url().unwrap().as_deref(),
            Some("git@github.com:user/other.git")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bind_commit_and_push_after_binding_local_remote() {
        if !git_available() {
            return;
        }
        let base =
            std::env::temp_dir().join(format!("deploycode-git-e2e-{}", uuid::Uuid::new_v4()));
        let work = base.join("work");
        let remote = base.join("remote.git");
        std::fs::create_dir_all(&work).unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg(&remote)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        std::fs::write(work.join("readme.md"), "hello").unwrap();

        let git = Git::open(&work).unwrap();
        assert!(!git.is_repo());
        git.set_remote_url(&remote.to_string_lossy()).unwrap();
        // 提交身份与签名在本地配置，不依赖运行环境的全局 Git 配置。
        git.run(&["config", "user.name", "DeployCode Test"]).unwrap();
        git.run(&["config", "user.email", "test@example.com"]).unwrap();
        git.run(&["config", "commit.gpgsign", "false"]).unwrap();

        let message = git.commit_all("init", false).unwrap();
        assert!(message.contains("init"));
        git.push().unwrap();

        let branch = git.current_branch().unwrap();
        let out = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(&remote)
            .arg("rev-parse")
            .arg("--verify")
            .arg(format!("refs/heads/{branch}"))
            .output()
            .unwrap();
        assert!(out.status.success(), "远端未收到推送的分支 {branch}");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn archive_worktree_includes_uncommitted_and_untracked_changes() {
        use std::io::Read;

        if !git_available() {
            return;
        }
        let base =
            std::env::temp_dir().join(format!("deploycode-git-worktree-{}", uuid::Uuid::new_v4()));
        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("tracked.txt"), "old\n").unwrap();
        std::fs::write(work.join(".gitignore"), "ignored.txt\n").unwrap();

        let git = Git::open(&work).unwrap();
        git.set_remote_url("https://github.com/user/repo.git").unwrap();
        git.run(&["config", "user.name", "DeployCode Test"]).unwrap();
        git.run(&["config", "user.email", "test@example.com"]).unwrap();
        git.run(&["config", "commit.gpgsign", "false"]).unwrap();
        git.commit_all("init", false).unwrap();

        // 未提交改动 + 未跟踪文件都应进入工作区快照，被忽略的文件不进入。
        std::fs::write(work.join("tracked.txt"), "changed\n").unwrap();
        std::fs::write(work.join("new.txt"), "brand-new\n").unwrap();
        std::fs::write(work.join("ignored.txt"), "secret\n").unwrap();

        let tar_path = base.join("out.tar");
        let gz_path = base.join("out.tar.gz");
        let size = git.archive_worktree(&tar_path, &gz_path).unwrap();
        assert!(size > 0);
        assert!(!tar_path.exists(), "打包完成后应删除中间 tar");

        let mut text = String::new();
        flate2::read::GzDecoder::new(File::open(&gz_path).unwrap())
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains("changed"), "快照应包含未提交改动");
        assert!(text.contains("new.txt"), "快照应包含未跟踪文件");
        assert!(!text.contains("secret"), "快照应跳过 .gitignore 忽略的文件");

        // 临时索引只是打包中间产物：真实暂存区仍保持原样（改动未被 add）。
        let status = git.run(&["status", "--porcelain"]).unwrap();
        assert!(status.contains("tracked.txt"), "status = {status}");
        assert!(!status.contains("A  tracked.txt"), "status = {status}");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn diff_file_handles_unborn_repo_and_untracked_files() {
        if !git_available() {
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("deploycode-git-diff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("first.txt"), "first\n").unwrap();

        let git = Git::open(&dir).unwrap();
        git.set_remote_url("https://github.com/user/repo.git").unwrap();

        // 空仓库也应读到初始分支名（否则绑定后界面会一直显示 "-"）。
        let branch = git.current_branch().unwrap();
        assert!(!branch.is_empty() && branch != "HEAD", "branch = {branch}");

        // 空仓库推送应给出明确提示，而不是 git 原始的 refspec 报错。
        let err = git.push().unwrap_err().to_string();
        assert!(err.contains("还没有可推送的提交"), "err = {err}");

        // 空仓库的未跟踪文件：应构造新增差异，而不是 `git diff HEAD` 报错。
        let diff = git.diff_file("first.txt").unwrap();
        assert!(diff.contains("+first"), "diff = {diff}");

        // 空仓库的已暂存文件：走 `--cached` 差异。
        git.run(&["add", "-A"]).unwrap();
        let diff = git.diff_file("first.txt").unwrap();
        assert!(diff.contains("+first"), "diff = {diff}");

        // 空仓库的文件名搜索与内容搜索仍走 git 路径。
        std::fs::write(dir.join("third.txt"), "needle\n").unwrap();
        assert!(git
            .find_files("third.txt", 10)
            .unwrap()
            .contains(&"third.txt".to_string()));
        assert!(git
            .search_content("needle", false, 10)
            .unwrap()
            .iter()
            .any(|hit| hit.path == "third.txt"));

        git.run(&["config", "user.name", "DeployCode Test"]).unwrap();
        git.run(&["config", "user.email", "test@example.com"]).unwrap();
        git.run(&["config", "commit.gpgsign", "false"]).unwrap();
        git.commit_all("init", false).unwrap();

        // 有 HEAD 后修改已跟踪文件。
        std::fs::write(dir.join("first.txt"), "first changed\n").unwrap();
        let diff = git.diff_file("first.txt").unwrap();
        assert!(
            diff.contains("-first") && diff.contains("+first changed"),
            "diff = {diff}"
        );

        // 有 HEAD 后新增未跟踪文件。
        std::fs::write(dir.join("second.txt"), "second\n").unwrap();
        let diff = git.diff_file("second.txt").unwrap();
        assert!(diff.contains("+second"), "diff = {diff}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_file_on_non_git_dir_reports_clear_error() {
        let dir =
            std::env::temp_dir().join(format!("deploycode-git-nodiff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "x").unwrap();

        let git = Git { path: dir.clone() };
        let err = git.diff_file("a.txt").unwrap_err().to_string();
        if git_available() {
            assert!(err.contains("尚未绑定 Git 仓库"), "err = {err}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_and_search_fall_back_without_git_repo() {
        let dir =
            std::env::temp_dir().join(format!("deploycode-git-walk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::write(dir.join("src/a.txt"), "first line\nHello World\n").unwrap();
        std::fs::write(dir.join("src/b.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("node_modules/pkg/a.txt"), "Hello World\n").unwrap();
        std::fs::write(dir.join(".git-placeholder"), "x").unwrap();

        let git = Git { path: dir.clone() };

        let files = git.find_files("a.txt", 10).unwrap();
        assert_eq!(files, vec!["src/a.txt".to_string()]);

        let hits = git.search_content("hello", false, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/a.txt");
        assert_eq!(hits[0].line, 2);
        assert_eq!(hits[0].text, "Hello World");

        assert!(git.search_content("HELLO", true, 10).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sensitive_path_detection_matches_common_secrets() {
        for path in [
            ".env",
            ".env.production",
            "docker/.env",
            "prod.env",
            "server.pem",
            "TLS.KEY",
            "id_rsa",
            "credentials.json",
            "secrets.yaml",
            ".npmrc",
            ".git-credentials",
            "app.p12",
        ] {
            assert!(is_sensitive_path(path), "{path} 应判定为敏感文件");
        }
        for path in [
            ".env.example",
            ".env.sample",
            ".env.template",
            ".env.dist",
            "id_rsa.pub",
            "src/main.rs",
            "readme.md",
            "package.json",
        ] {
            assert!(!is_sensitive_path(path), "{path} 不应判定为敏感文件");
        }
    }

    #[test]
    fn commit_all_blocks_sensitive_files_until_allowed() {
        if !git_available() {
            return;
        }
        let dir =
            std::env::temp_dir().join(format!("deploycode-git-secret-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = Git::open(&dir).unwrap();
        git.run(&["init"]).unwrap();
        std::fs::write(dir.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(dir.join("readme.md"), "hi\n").unwrap();

        assert_eq!(git.sensitive_changes().unwrap(), vec![".env".to_string()]);
        let err = git.commit_all("init", false).unwrap_err().to_string();
        assert!(err.contains(".env"), "err = {err}");

        git.run(&["config", "user.name", "DeployCode Test"]).unwrap();
        git.run(&["config", "user.email", "test@example.com"]).unwrap();
        git.run(&["config", "commit.gpgsign", "false"]).unwrap();
        git.commit_all("init", true).unwrap();
        assert!(git.status().unwrap().changes.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
