use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{CoreError, Result};
use crate::models::{ServerConfig, SshAuth};

/// 远端命令输出的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Stdout,
    Stderr,
}

struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        // 轻量级内部工具：信任所有主机公钥。
        Ok(true)
    }
}

/// 一个已连接的 SSH 会话，提供命令执行与文件上传能力。
pub struct SshClient {
    session: Handle<ClientHandler>,
    label: String,
}

impl SshClient {
    /// 建立连接并完成认证。
    pub async fn connect(server: &ServerConfig, timeout_secs: u64) -> Result<Self> {
        let label = format!("{}@{}:{}", server.username, server.host, server.port);
        let config = Arc::new(client::Config {
            inactivity_timeout: None,
            ..Default::default()
        });

        let timeout = Duration::from_secs(timeout_secs.max(1));
        let mut session = tokio::time::timeout(
            timeout,
            client::connect(config, (server.host.as_str(), server.port), ClientHandler),
        )
        .await
        .map_err(|_| CoreError::ssh(format!("连接 {label} 超时")))?
        .map_err(|e| CoreError::ssh(format!("连接 {label} 失败: {e}")))?;

        let authenticated = match &server.auth {
            SshAuth::Password { password } => session
                .authenticate_password(server.username.clone(), password.clone())
                .await
                .map_err(|e| CoreError::ssh(format!("认证失败: {e}")))?
                .success(),
            SshAuth::PrivateKey {
                key_path,
                passphrase,
            } => {
                let key = load_secret_key(key_path, passphrase.as_deref()).map_err(|e| {
                    CoreError::ssh(format!("读取私钥失败 {}: {e}", key_path))
                })?;
                let hash_alg = session
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|e| CoreError::ssh(format!("协商密钥算法失败: {e}")))?
                    .flatten();
                session
                    .authenticate_publickey(
                        server.username.clone(),
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                    )
                    .await
                    .map_err(|e| CoreError::ssh(format!("认证失败: {e}")))?
                    .success()
            }
        };

        if !authenticated {
            return Err(CoreError::ssh(format!(
                "{label} 认证失败，请检查用户名、密码或私钥"
            )));
        }

        Ok(Self {
            session,
            label,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// 执行远端命令并捕获完整输出（同时包含 stdout 和 stderr）。
    pub async fn exec_capture(&self, command: &str, timeout_secs: u64) -> Result<(i32, String)> {
        let mut output = String::new();
        let mut sink = |kind: OutputKind, line: String| {
            if kind == OutputKind::Stderr {
                output.push_str("[stderr] ");
            }
            output.push_str(&line);
            output.push('\n');
        };
        let code = self.exec_stream(command, timeout_secs, &mut sink).await?;
        Ok((code, output))
    }

    /// 执行远端命令，按行流式回调输出。
    pub async fn exec_stream(
        &self,
        command: &str,
        timeout_secs: u64,
        on_output: &mut (dyn FnMut(OutputKind, String) + Send),
    ) -> Result<i32> {
        let timeout = Duration::from_secs(timeout_secs.max(1));
        let result = tokio::time::timeout(
            timeout,
            self.exec_stream_inner(command, on_output),
        )
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(CoreError::ssh(format!(
                "命令执行超时（{} 秒）: {command}",
                timeout.as_secs()
            ))),
        }
    }

    async fn exec_stream_inner(
        &self,
        command: &str,
        on_output: &mut (dyn FnMut(OutputKind, String) + Send),
    ) -> Result<i32> {
        let mut channel = self
            .session
            .channel_open_session()
            .await
            .map_err(|e| CoreError::ssh(format!("打开 SSH 通道失败: {e}")))?;
        channel
            .exec(true, command)
            .await
            .map_err(|e| CoreError::ssh(format!("执行命令失败: {e}")))?;

        let mut exit_code: i32 = -1;
        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();

        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    stdout_buf.push_str(&String::from_utf8_lossy(&data));
                    drain_lines(&mut stdout_buf, OutputKind::Stdout, on_output);
                }
                ChannelMsg::ExtendedData { data, ext } => {
                    let _ = ext;
                    stderr_buf.push_str(&String::from_utf8_lossy(&data));
                    drain_lines(&mut stderr_buf, OutputKind::Stderr, on_output);
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = exit_status as i32;
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }

        flush_rest(&mut stdout_buf, OutputKind::Stdout, on_output);
        flush_rest(&mut stderr_buf, OutputKind::Stderr, on_output);
        Ok(exit_code)
    }

    /// 递归创建远端目录。
    pub async fn mkdir_p(&self, dir: &str) -> Result<()> {
        let command = format!("mkdir -p {}", crate::process::shell_quote(dir));
        let (code, output) = self.exec_capture(&command, 60).await?;
        if code != 0 {
            return Err(CoreError::ssh(format!(
                "创建远端目录失败 {}: {}",
                dir,
                output.trim()
            )));
        }
        Ok(())
    }

    /// 通过 SFTP 上传文件，并回调上传进度。
    pub async fn upload_file(
        &self,
        local: &Path,
        remote: &str,
        on_progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<()> {
        let channel = self
            .session
            .channel_open_session()
            .await
            .map_err(|e| CoreError::ssh(format!("打开 SFTP 通道失败: {e}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| CoreError::ssh(format!("启动 SFTP 子系统失败: {e}")))?;

        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| CoreError::ssh(format!("初始化 SFTP 失败: {e}")))?;
        sftp.set_timeout(120);

        let total = std::fs::metadata(local)
            .map_err(|e| CoreError::io_path(local, e))?
            .len();

        let mut remote_file = sftp
            .create(remote.to_string())
            .await
            .map_err(|e| CoreError::ssh(format!("创建远端文件失败 {remote}: {e}")))?;

        let mut file = tokio::fs::File::open(local)
            .await
            .map_err(|e| CoreError::io_path(local, e))?;
        let mut buffer = vec![0u8; 256 * 1024];
        let mut sent = 0u64;

        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            remote_file
                .write_all(&buffer[..read])
                .await
                .map_err(|e| CoreError::ssh(format!("上传数据失败: {e}")))?;
            sent += read as u64;
            on_progress(sent, total);
        }

        remote_file
            .flush()
            .await
            .map_err(|e| CoreError::ssh(format!("刷新远端文件失败: {e}")))?;
        let _ = remote_file.shutdown().await;
        let _ = sftp.close().await;
        Ok(())
    }

    /// 主动断开连接。
    pub async fn disconnect(&self) {
        let _ = self
            .session
            .disconnect(Disconnect::ByApplication, "bye", "en")
            .await;
    }
}

fn drain_lines(
    buffer: &mut String,
    kind: OutputKind,
    on_output: &mut (dyn FnMut(OutputKind, String) + Send),
) {
    while let Some(pos) = buffer.find('\n') {
        let line: String = buffer.drain(..=pos).collect();
        on_output(kind, line.trim_end_matches(['\r', '\n']).to_string());
    }
}

fn flush_rest(
    buffer: &mut String,
    kind: OutputKind,
    on_output: &mut (dyn FnMut(OutputKind, String) + Send),
) {
    if !buffer.is_empty() {
        on_output(kind, buffer.trim_end_matches(['\r', '\n']).to_string());
        buffer.clear();
    }
}
