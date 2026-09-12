use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{CoreError, Result};

/// 本地命令输出。
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.code == 0
    }

    pub fn combined(&self) -> String {
        let mut text = self.stdout.trim_end().to_string();
        let err = self.stderr.trim_end();
        if !err.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(err);
        }
        text
    }
}

/// 构造命令，Windows 下隐藏控制台窗口，避免 GUI 弹出黑框。
fn base_command(program: &str, args: &[String], cwd: Option<&Path>) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 执行本地命令，不抛出非零退出码错误。
pub fn run(program: &str, args: &[String], cwd: Option<&Path>) -> Result<CommandOutput> {
    let output = base_command(program, args, cwd).output().map_err(|e| {
        CoreError::Process(format!("无法执行 `{program}`: {e}（请确认该命令已安装并在 PATH 中）"))
    })?;

    Ok(CommandOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// 带超时的本地命令执行。超时会杀掉子进程及其后代并返回错误，避免网络操作永久卡死。
pub fn run_timeout(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut cmd = base_command(program, args, cwd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 新建进程组，超时可整组终止（含子进程拉起的孙进程，如 git 的 ssh/凭据助手）。
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        CoreError::Process(format!("无法执行 `{program}`: {e}（请确认该命令已安装并在 PATH 中）"))
    })?;

    let out_thread = child.stdout.take().map(|mut stdout| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            buf
        })
    });
    let err_thread = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_process_tree(&mut child);
                    // 读取线程不 join：孙进程可能仍持有管道，join 会把"超时"变成永久阻塞；
                    // 进程组/进程树被终止后管道会关闭，线程自行退出。
                    drop(out_thread);
                    drop(err_thread);
                    return Err(CoreError::Process(format!(
                        "`{program}` 执行超时（{} 秒），已终止",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                kill_process_tree(&mut child);
                return Err(CoreError::Process(format!("`{program}` 执行失败: {e}")));
            }
        }
    };

    let stdout = out_thread.and_then(|thread| thread.join().ok()).unwrap_or_default();
    let stderr = err_thread.and_then(|thread| thread.join().ok()).unwrap_or_default();
    Ok(CommandOutput {
        code: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// 终止子进程及其后代。Unix 下按进程组终止；Windows 下用 taskkill /T 终止进程树。
fn kill_process_tree(child: &mut std::process::Child) {
    let pid = child.id();

    #[cfg(unix)]
    unsafe {
        // run_timeout 已让子进程带头新建进程组，负 PID 表示整组终止。
        libc::kill(-(pid as i32), libc::SIGKILL);
    }

    #[cfg(windows)]
    {
        // 复用 base_command 以带上 CREATE_NO_WINDOW，避免 GUI 下闪控制台窗口。
        let args = vec![
            "/F".to_string(),
            "/T".to_string(),
            "/PID".to_string(),
            pid.to_string(),
        ];
        let _ = base_command("taskkill", &args, None)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = child.kill();
    }

    // 兜底：若上面的树终止未生效（如 taskkill 不可用），至少终止直接子进程。
    let _ = child.kill();
    let _ = child.wait();
}

/// 执行本地命令，非零退出码返回错误。
pub fn run_checked(program: &str, args: &[String], cwd: Option<&Path>) -> Result<String> {
    let output = run(program, args, cwd)?;
    if !output.success() {
        let detail = output.combined();
        return Err(CoreError::Process(format!(
            "`{program}` 执行失败（退出码 {}）：{}",
            output.code,
            if detail.is_empty() { "无输出" } else { &detail }
        )));
    }
    Ok(output.stdout)
}

/// 将字符串安全地包成 shell 参数（用于远端 shell 命令拼接）。
/// 注意：`~` 不在安全集合内，会被单引号包裹而不会被 shell 展开，保证字面量语义。
pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._/:@%^+,=".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 规范化本地路径：解析为绝对路径，并去掉 Windows 上的 `\\?\` 前缀。
pub fn canonicalize_path(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(canonical) => strip_unc_prefix(canonical),
        Err(_) => path.to_path_buf(),
    }
}

fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy().into_owned();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_string());
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_quotes_tilde_and_escapes() {
        assert_eq!(shell_quote("1.2.3.4"), "1.2.3.4");
        assert_eq!(shell_quote("/srv/app"), "/srv/app");
        assert_eq!(shell_quote("~/app"), "'~/app'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn run_timeout_returns_promptly_and_kills_child() {
        #[cfg(windows)]
        let (program, args) = (
            "cmd",
            vec!["/C".to_string(), "ping -n 60 127.0.0.1 >NUL".to_string()],
        );
        #[cfg(unix)]
        let (program, args) = ("sh", vec!["-c".to_string(), "sleep 60".to_string()]);

        let started = Instant::now();
        let result = run_timeout(program, &args, None, Duration::from_millis(300));
        assert!(result.is_err(), "长命令应超时");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "超时后应尽快返回，实际耗时 {:?}",
            started.elapsed()
        );
    }
}
