use deploy_core::{CoreError, Git, Result, Store};

use crate::cli::*;
use crate::output;

use super::{open_store, print_json};

pub(super) fn branch_command(cli: &Cli, command: &BranchCommand) -> Result<()> {
    let store = open_store(cli)?;
    let config = store.load_config()?;
    match command {
        BranchCommand::List { repo, all } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let branches = git.branches(*all)?;
            if cli.json {
                return print_json(&branches);
            }
            for branch in branches {
                let marker = if branch.is_current { "*" } else { " " };
                let mut tags = Vec::new();
                if branch.is_remote {
                    tags.push("远程");
                }
                if let Some(upstream) = &branch.upstream {
                    if !branch.is_remote {
                        tags.push(upstream.as_str());
                    }
                }
                let suffix = if tags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", tags.join(" · "))
                };
                println!(
                    "{marker} {:<36} {:<17} {}{suffix}",
                    branch.name, branch.last_commit_date, branch.last_commit_subject
                );
            }
            Ok(())
        }
        BranchCommand::Switch { repo, branch } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.checkout(branch)?;
            output::success(message);
            Ok(())
        }
        BranchCommand::Create {
            repo,
            name,
            from,
            no_checkout,
        } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.create_branch(name, from.as_deref(), !no_checkout)?;
            output::success(message);
            Ok(())
        }
        BranchCommand::Delete {
            repo,
            branch,
            force,
        } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            if git.current_branch().unwrap_or_default() == *branch {
                return Err(CoreError::git("不能删除当前所在分支"));
            }
            let message = git.delete_branch(branch, *force)?;
            output::success(message);
            Ok(())
        }
        BranchCommand::Log { repo, limit } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let commits = git.log(*limit)?;
            if cli.json {
                return print_json(&commits);
            }
            for commit in commits {
                println!(
                    "{:<10} {:<17} {:<14} {}",
                    commit.short, commit.date, commit.author, commit.subject
                );
            }
            Ok(())
        }
        BranchCommand::Status { repo } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let status = git.status()?;
            if cli.json {
                return print_json(&status);
            }
            let upstream = status
                .upstream
                .as_deref()
                .map(|value| format!(" -> {value}"))
                .unwrap_or_default();
            println!("分支: {}{upstream}", status.branch);
            if status.ahead > 0 || status.behind > 0 {
                println!("领先 {} / 落后 {}", status.ahead, status.behind);
            }
            if status.changes.is_empty() {
                output::dim("工作区干净");
            } else {
                for change in status.changes {
                    println!("  {:<8} {}", change.status, change.path);
                }
            }
            Ok(())
        }
        BranchCommand::Commit {
            repo,
            message,
            allow_sensitive,
        } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let result = git.commit_all(message, *allow_sensitive)?;
            output::success(result);
            Ok(())
        }
        BranchCommand::Reset { repo, rev, yes } => {
            if !yes {
                return Err(CoreError::git(
                    "该操作会丢弃未提交的工作区改动，确认请加 --yes",
                ));
            }
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.reset_hard(rev)?;
            output::success(message);
            Ok(())
        }
        BranchCommand::Fetch { repo } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.fetch()?;
            output::info(message);
            Ok(())
        }
        BranchCommand::Pull { repo } => {
            let repo = Store::find_repo(&config, repo)?.clone();
            let git = Git::open(&repo.path)?;
            let message = git.pull()?;
            output::info(message);
            Ok(())
        }
    }
}
