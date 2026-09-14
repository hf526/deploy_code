use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::error::{CoreError, Result};
use crate::ssh::OutputKind;

/// 当前存活的本地子进程（用于应用退出时统一终止，避免残留构建/上传进程）。
fn running_children() -> &'static Mutex<HashSet<u32>> {
    static CHILDREN: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
    CHILDREN.get_or_init(|| Mutex::new(HashSet::new()))
}

fn track_child(pid: u32) {
    running_children()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(pid);
}

fn untrack_child(pid: u32) {
    running_children()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&pid);
}

/// 终止所有仍在运行的本地子进程（应用退出时调用；Unix 按进程组，Windows 用 taskkill /T）。
pub fn kill_all_children() {
    let pids: Vec<u32> = running_children()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
        .collect();
    for pid in pids {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
            libc::kill(pid as i32, libc::SIGKILL);
        }
        #[cfg(windows)]
        {
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
            let _ = pid;
        }
    }
}

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
    run_with_env(program, args, cwd, &[])
}

/// 执行本地命令（附加环境变量），不抛出非零退出码错误。
pub fn run_with_env(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    envs: &[(String, String)],
) -> Result<CommandOutput> {
    // 与 run_timeout/run_stream 一致登记子进程，应用退出时才能统一终止（Git 本地操作也走这里）。
    let mut cmd = base_command(program, args, cwd);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            CoreError::Process(format!(
                "无法执行 `{program}`: {e}（请确认该命令已安装并在 PATH 中）"
            ))
        })?;
    let pid = child.id();
    track_child(pid);
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => {
            untrack_child(pid);
            return Err(CoreError::Process(format!("`{program}` 执行失败: {e}")));
        }
    };
    untrack_child(pid);

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
    let pid = child.id();
    track_child(pid);

    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>();
    let out_thread = child.stdout.take().map(|mut stdout| {
        let tx = out_tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            let _ = tx.send(buf);
        })
    });
    let (err_tx, err_rx) = mpsc::channel::<Vec<u8>>();
    let err_thread = child.stderr.take().map(|mut stderr| {
        let tx = err_tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            let _ = tx.send(buf);
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_process_tree(&mut child);
                    untrack_child(pid);
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
                untrack_child(pid);
                return Err(CoreError::Process(format!("`{program}` 执行失败: {e}")));
            }
        }
    };

    // 子进程已退出，但孙进程可能仍持有管道写端，读线程的 join 可能长期阻塞：
    // 用带超时的通道收结果，超时则放弃剩余输出并继续。
    const READ_GRACE: Duration = Duration::from_secs(5);
    let stdout = out_rx.recv_timeout(READ_GRACE).unwrap_or_default();
    let stderr = err_rx.recv_timeout(READ_GRACE).unwrap_or_default();
    untrack_child(pid);
    drop(out_thread);
    drop(err_thread);
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

/// 本地命令流式执行：按行回调输出（stdout/stderr），支持超时终止进程树。返回退出码。
pub fn run_stream(
    program: &str,
    args: &[String],
    cwd: Option<&Path>,
    envs: &[(String, String)],
    timeout: Duration,
    on_line: &mut (dyn FnMut(OutputKind, String) + Send),
) -> Result<i32> {
    let mut cmd = base_command(program, args, cwd);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        CoreError::Process(format!("无法执行 `{program}`: {e}（请确认该命令已安装并在 PATH 中）"))
    })?;
    let pid = child.id();
    track_child(pid);

    let (tx, rx) = mpsc::channel::<(OutputKind, String)>();
    let out_tx = tx.clone();
    let out_thread = child
        .stdout
        .take()
        .map(|stdout| std::thread::spawn(move || pump_lines(stdout, OutputKind::Stdout, out_tx)));
    let err_tx = tx.clone();
    let err_thread = child
        .stderr
        .take()
        .map(|stderr| std::thread::spawn(move || pump_lines(stderr, OutputKind::Stderr, err_tx)));
    drop(tx);

    let deadline = Instant::now() + timeout;
    let status = loop {
        // 每轮最多消费固定条数：高输出量时不能无限排空通道，否则永远检查不到超时。
        for _ in 0..512 {
            match rx.try_recv() {
                Ok((kind, line)) => on_line(kind, line),
                Err(_) => break,
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_process_tree(&mut child);
                    untrack_child(pid);
                    // 不 join 读取线程：孙进程可能仍持有管道导致永久阻塞。
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
                untrack_child(pid);
                drop(out_thread);
                drop(err_thread);
                return Err(CoreError::Process(format!("`{program}` 执行失败: {e}")));
            }
        }
    };

    // 收尾：把剩余日志送完，但最多等 2 秒，避免管道被孙进程持有导致卡死。
    let drain_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if Instant::now() >= drain_deadline {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok((kind, line)) => on_line(kind, line),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    untrack_child(pid);
    drop(out_thread);
    drop(err_thread);
    Ok(status.code().unwrap_or(-1))
}

/// 使用系统 shell 执行命令字符串（构建命令等），流式输出。
pub fn run_shell_stream(
    command: &str,
    cwd: Option<&Path>,
    envs: &[(String, String)],
    timeout: Duration,
    on_line: &mut (dyn FnMut(OutputKind, String) + Send),
) -> Result<i32> {
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe".to_string(),
        vec!["/C".to_string(), command.to_string()],
    );
    #[cfg(not(windows))]
    let (program, args) = (
        "sh".to_string(),
        vec!["-c".to_string(), command.to_string()],
    );
    run_stream(&program, &args, cwd, envs, timeout, on_line)
}

/// 从管道按行读取并通过通道发送（在主线程回调，避免回调跨线程竞争）。
fn pump_lines(pipe: impl Read, kind: OutputKind, tx: mpsc::Sender<(OutputKind, String)>) {
    let mut reader = BufReader::new(pipe);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\r', '\n']).to_string();
                if tx.send((kind, line)).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
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

/// 计算本地文件的 SHA-256（十六进制小写），用于上传完整性校验。
pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path).map_err(|e| CoreError::io_path(path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| CoreError::io_path(path, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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

    #[test]
    fn sha256_file_matches_known_digest() {
        let dir = std::env::temp_dir().join(format!("deploycode-sha-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.bin");
        std::fs::write(&file, b"hello").unwrap();
        // echo -n hello | sha256sum
        assert_eq!(
            sha256_file(&file).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
