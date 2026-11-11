//! 部署失败诊断：把脱敏后的失败上下文交给 OpenAI 兼容接口，返回结构化的原因与修复建议。
//!
//! 安全边界（改这个模块前先读完）：
//! - 只发送记录里的展示字段与**脱敏后的日志尾部**；不发送服务器地址、用户名、密码、
//!   密钥、env 文件内容或任何 `Settings`。
//! - 模型只能从 [`DiagnosticCommandId`] / [`AiFixAction`] 的白名单里选动作，不能自带命令；
//!   一旦判定为凭据类问题，后端强制清空动作，只提醒用户手动处理。
//! - 预览与发送共用同一份上下文拼装，用户看到的「将发送的内容」就是真正发出去的内容。

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::git::mask_remote_urls_in_text;
use crate::models::{
    now_string, AiConfidence, AiDiagnosis, AiFixAction, DeployRecord, DeployStatus,
    DiagnosticCommandId,
};
use crate::process::{run_timeout, shell_quote};
use crate::ssh::SshClient;
use crate::store::Store;

/// 模型单次请求的超时（秒）：诊断是用户主动等待的短操作，再长就该报错而不是干等。
const MODEL_TIMEOUT_SECS: u64 = 60;
/// 日志最多送 24 KiB：失败原因几乎总在尾部，整份构建日志既费 token 也更容易带出无关信息。
const MAX_LOG_BYTES: usize = 24 * 1024;
/// 单条诊断命令的输出上限：`ls` 一个巨大的目录可能吐几十 MB。
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const DIAGNOSTIC_TIMEOUT_SECS: u64 = 20;
const MAX_STEPS: usize = 8;
const MAX_ACTIONS: usize = 3;

const DEFAULT_CREDENTIAL_NOTE: &str = "失败可能与密码、密钥或 Token 等凭据有关。为避免把凭据交给模型或误改密钥文件，本次不提供自动修复动作，请手动核对该服务器的认证配置。";

const SYSTEM_PROMPT: &str = r#"你是 DeployCode 的部署故障诊断助手。根据用户提供的部署上下文与脱敏日志，判断失败原因并给出可执行的修复建议。

只输出一个 JSON 对象，不要 Markdown、不要代码围栏、不要额外说明。字段如下：
{
  "summary": "一句话结论",
  "likelyCause": "失败原因分析",
  "confidence": "high | medium | low",
  "steps": ["按顺序排列的修复步骤，最多 6 条"],
  "actions": [{"type": "redeploy"}, {"type": "openTerminal"}, {"type": "runDiagnostic", "command": "diskUsage" | "memory" | "failedServices" | "targetDir" | "docker"}],
  "credentialSuspect": false,
  "credentialNote": null
}

规则：
1. 全部使用简体中文。
2. actions 只能从上面列出的类型中选择，最多 3 个；不要编造其它动作或命令。
3. 如果失败可能由密码、密钥、Token、SSH 凭据或任何凭据问题导致：把 credentialSuspect 设为 true，actions 必须为空数组，并在 credentialNote 中说明用户需要手动检查什么。
4. 不要建议修改 .env 或任何密钥文件，也不要输出任何密钥内容。
5. 如果日志不足以判断，confidence 用 low，并在 steps 中给出进一步排查建议。
6. 日志里被替换成 *** 的内容是已脱敏的凭据，不要试图还原。"#;

/// 预览：真正要发给模型的部署上下文（已脱敏）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosisPreview {
    pub context: String,
    /// 参与分析的脱敏日志字节数。
    pub log_bytes: usize,
    pub total_bytes: usize,
}

/// 一条只读诊断命令的执行结果（只在本机展示，不回灌给模型）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticOutput {
    pub command: DiagnosticCommandId,
    pub code: i32,
    pub output: String,
    pub truncated: bool,
}

/// 拼装「将发送给模型」的上下文；`preview` 与 `analyze` 必须共用这一份。
fn build_context(record: &DeployRecord) -> (String, usize) {
    let redacted_log = redact_for_llm(&record.log);
    let (log_tail, _) = tail(&redacted_log, MAX_LOG_BYTES);
    let error = record
        .error
        .as_deref()
        .map(redact_for_llm)
        .map(|value| clip(&value, 4000))
        .unwrap_or_else(|| "（无错误摘要）".to_string());
    let scripts = if record.scripts.is_empty() {
        "（自动执行脚本目录下的全部 .sh）".to_string()
    } else {
        record.scripts.join(", ")
    };
    let context = format!(
        "仓库：{}\n分支：{}\n版本：{}\n提交：{} {}\n服务器：{}\n目标目录：{}\n脚本目录：{}\n执行脚本：{}\n是否执行脚本：{}\n开始时间：{}\n结束时间：{}\n错误：{}\n\n日志（已脱敏，以下为尾部 {} 字节）：\n{}",
        record.repo_name,
        record.branch,
        if record.rev.is_empty() {
            "（当前工作区）"
        } else {
            record.rev.as_str()
        },
        record.commit_short,
        record.commit_subject,
        record.server_name,
        record.target_dir,
        record.script_dir,
        scripts,
        if record.run_scripts { "是" } else { "否" },
        record.started_at,
        record.finished_at.as_deref().unwrap_or("（仍在进行）"),
        error,
        log_tail.len(),
        log_tail,
    );
    (context, log_tail.len())
}

/// 取文本尾部，且不切断 UTF-8 字符边界。
fn tail(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (&text[start..], true)
}

/// 发给模型前的脱敏：私钥块、ANSI 控制符、`key=value` 形式的凭据、URL 内嵌口令。
pub fn redact_for_llm(text: &str) -> String {
    let text = mask_secret_assignments(&strip_ansi(&strip_private_key_blocks(text)));
    mask_remote_urls_in_text(&text)
}

fn strip_private_key_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_block = false;
    for line in text.lines() {
        let begin = line.contains("-----BEGIN ") && line.contains("PRIVATE KEY-----");
        if !in_block && begin {
            in_block = true;
            out.push_str("[已隐去私钥内容]\n");
            continue;
        }
        if in_block {
            if line.contains("-----END ") && line.contains("PRIVATE KEY-----") {
                in_block = false;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// 去掉终端颜色 / 光标控制序列：日志里混着 ANSI 既费 token，也会让模型把转义码当成内容。
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            out.push(ch);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI：参数中间字节到 0x40..=0x7E 的终止符为止。
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC：BEL 或 ESC \ 结束。
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "token",
    "secret",
    "api_key",
    "apikey",
    "api-key",
    "access_key",
    "access-key",
    "secret_key",
    "secret-key",
    "private_key",
    "authorization",
];

fn mask_secret_assignments(text: &str) -> String {
    text.lines()
        .map(mask_secret_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 只处理「关键字 + 分隔符 + 值」的形态（`token=xxx`、`password: xxx`、
/// `Authorization: Bearer xxx`）。没有分隔符的普通叙述（如「token 已过期」）保持原样。
fn mask_secret_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        let Some((value_start, value_end)) = next_secret_span(rest) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..value_start]);
        out.push_str("***");
        rest = &rest[value_end..];
    }
    out
}

fn next_secret_span(line: &str) -> Option<(usize, usize)> {
    let lower = line.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    for key in SECRET_KEYS {
        let mut from = 0;
        while let Some(rel) = lower[from..].find(key) {
            let pos = from + rel;
            let before_ok = pos == 0
                || !line[..pos]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
            let after = pos + key.len();
            let after_ok = !line[after..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
            if !(before_ok && after_ok) {
                from = after;
                continue;
            }
            let Some((separator, separator_len)) = find_separator(line, after) else {
                from = after;
                continue;
            };
            let mut value_start = skip_spaces(line, separator + separator_len);
            // `Authorization: Bearer xxx`：真正要遮的是 Bearer 后面那段。
            if let Some(after_bearer) = strip_bearer_prefix(&line[value_start..]) {
                value_start += after_bearer;
            }
            if value_start >= line.len() {
                from = after;
                continue;
            }
            let value_end = find_value_end(line, value_start);
            if value_end <= value_start {
                from = after;
                continue;
            }
            if best.is_none_or(|(start, _)| value_start < start) {
                best = Some((value_start, value_end));
            }
            break;
        }
    }
    best
}

fn find_separator(line: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut index = from;
    // 放行 `AWS_SECRET_ACCESS_KEY` / `access_key_id` 这类「关键字 + _后缀」形态；
    // 只接受 `_`/`-` 开头的后缀，避免把 `secretary` 这种普通单词吃进来。
    if index < bytes.len() && (bytes[index] == b'_' || bytes[index] == b'-') {
        while index < bytes.len()
            && (bytes[index].is_ascii_alphanumeric()
                || bytes[index] == b'_'
                || bytes[index] == b'-')
        {
            index += 1;
        }
    }
    while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
        index += 1;
    }
    match bytes.get(index) {
        Some(b':') | Some(b'=') => Some((index, 1)),
        _ => None,
    }
}

fn skip_spaces(line: &str, from: usize) -> usize {
    let bytes = line.as_bytes();
    let mut index = from;
    while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
        index += 1;
    }
    index
}

fn strip_bearer_prefix(value: &str) -> Option<usize> {
    let head = value.get(..6)?;
    if !head.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let start = skip_spaces(value, 6);
    (start > 6).then_some(start)
}

fn find_value_end(line: &str, start: usize) -> usize {
    let first = line[start..].chars().next();
    if let Some(quote) = first.filter(|c| *c == '"' || *c == '\'') {
        return line[start + quote.len_utf8()..]
            .find(quote)
            .map(|offset| start + quote.len_utf8() + offset + quote.len_utf8())
            .unwrap_or(line.len());
    }
    line[start..]
        .find(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '&' | '}' | ')'))
        .map(|offset| start + offset)
        .unwrap_or(line.len())
}

fn endpoint(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

/// 临时文件守卫：请求体与请求头里有一份 Key，任何返回路径都必须删掉。
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_temp(name: &str, content: &str) -> Result<TempFile> {
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, content).map_err(|error| CoreError::io_path(&path, error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(TempFile(path))
}

fn build_request_json(model: &str, context: &str) -> String {
    serde_json::json!({
        "model": model,
        "temperature": 0.2,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": context}
        ]
    })
    .to_string()
}

async fn call_model(base_url: &str, api_key: &str, request: &str) -> Result<String> {
    let id = crate::models::new_id();
    // Key 走请求头文件而不是 argv：curl 存活期间 argv 对同机进程可见，一次诊断要挂几十秒。
    let headers = write_temp(
        &format!("deploycode-ai-{id}.headers"),
        &format!(
            "Content-Type: application/json\nAuthorization: Bearer {}\n",
            api_key.trim()
        ),
    )?;
    let body = write_temp(&format!("deploycode-ai-{id}.json"), request)?;
    let args = vec![
        "-sS".to_string(),
        "--max-time".to_string(),
        MODEL_TIMEOUT_SECS.to_string(),
        "-H".to_string(),
        format!("@{}", headers.0.display()),
        "--data-binary".to_string(),
        format!("@{}", body.0.display()),
        "-w".to_string(),
        "\n%{http_code}".to_string(),
        endpoint(base_url),
    ];
    let output = tokio::task::spawn_blocking(move || {
        run_timeout(
            "curl",
            &args,
            None,
            Duration::from_secs(MODEL_TIMEOUT_SECS + 5),
        )
    })
    .await
    .map_err(|error| CoreError::diagnosis(format!("请求任务异常结束：{error}")))??;

    if output.code != 0 {
        return Err(CoreError::diagnosis(format!(
            "请求未完成（curl 退出码 {}）：{}",
            output.code,
            clip(&output.stderr, 300)
        )));
    }
    let (body, status) = split_http_code(&output.stdout)?;
    if !(200..300).contains(&status) {
        return Err(CoreError::diagnosis(format!(
            "接口返回 HTTP {status}：{}",
            clip(body, 500)
        )));
    }
    extract_content(body)
}

fn split_http_code(output: &str) -> Result<(&str, u16)> {
    let trimmed = output.trim_end_matches(['\r', '\n']);
    let (body, code) = trimmed
        .rsplit_once('\n')
        .ok_or_else(|| CoreError::diagnosis("接口响应缺少 HTTP 状态码"))?;
    let status = code
        .trim()
        .parse::<u16>()
        .map_err(|_| CoreError::diagnosis(format!("HTTP 状态码无法解析：{code}")))?;
    Ok((body, status))
}

fn extract_content(body: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| CoreError::diagnosis(format!("接口返回的不是 JSON：{error}")))?;
    value
        .pointer("/choices/0/message/content")
        .and_then(|item| item.as_str())
        .map(str::to_string)
        .ok_or_else(|| CoreError::diagnosis("接口响应缺少 choices[0].message.content"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireDiagnosis {
    #[serde(default)]
    summary: String,
    // 兼容模型偶尔写出的 snake_case：动作白名单之外的部分宁可多认一种写法。
    #[serde(default, alias = "likely_cause")]
    likely_cause: String,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    steps: Vec<String>,
    /// 逐条按 `Value` 解析：模型偶尔会把动作写成字符串或带额外字段，
    /// 单个动作不合法只该被丢弃，不该让整次诊断失败。
    #[serde(default)]
    actions: Vec<serde_json::Value>,
    #[serde(default, alias = "credential_suspect")]
    credential_suspect: bool,
    #[serde(default, alias = "credential_note")]
    credential_note: Option<String>,
}

fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

/// 把模型输出收敛成可信结构：动作只认白名单、凭据类怀疑清空动作、文本长度限幅。
fn parse_diagnosis(content: &str, model: &str, log_bytes: usize) -> Result<AiDiagnosis> {
    let json = extract_json(content)
        .ok_or_else(|| CoreError::diagnosis("模型没有返回 JSON 对象"))?;
    let wire: WireDiagnosis = serde_json::from_str(json)
        .map_err(|error| CoreError::diagnosis(format!("模型返回的 JSON 无法解析：{error}")))?;

    let mut actions: Vec<AiFixAction> = Vec::new();
    for action in wire.actions {
        let (kind, command) = match &action {
            serde_json::Value::String(kind) => (kind.as_str(), None),
            serde_json::Value::Object(map) => (
                map.get("type").and_then(|value| value.as_str()).unwrap_or_default(),
                map.get("command").and_then(|value| value.as_str()),
            ),
            _ => continue,
        };
        let parsed = match kind {
            "redeploy" => Some(AiFixAction::Redeploy),
            "openTerminal" | "open_terminal" => Some(AiFixAction::OpenTerminal),
            "runDiagnostic" | "run_diagnostic" => {
                command.and_then(parse_command).map(|command| AiFixAction::RunDiagnostic { command })
            }
            _ => None,
        };
        if let Some(action) = parsed {
            if !actions.contains(&action) {
                actions.push(action);
            }
        }
        if actions.len() >= MAX_ACTIONS {
            break;
        }
    }

    let credential_suspect = wire.credential_suspect;
    let credential_note = if credential_suspect {
        // 红线：凭据问题不提供任何自动动作，避免模型建议去改密钥文件或把口令读出来。
        actions.clear();
        Some(
            wire.credential_note
                .filter(|note| !note.trim().is_empty())
                .map(|note| clip(&note, 1000))
                .unwrap_or_else(|| DEFAULT_CREDENTIAL_NOTE.to_string()),
        )
    } else {
        wire.credential_note
            .filter(|note| !note.trim().is_empty())
            .map(|note| clip(&note, 1000))
    };

    let summary = clip(&wire.summary, 500);
    let likely_cause = clip(&wire.likely_cause, 2000);
    let steps: Vec<String> = wire
        .steps
        .into_iter()
        .map(|step| clip(&step, 500))
        .filter(|step| !step.is_empty())
        .take(MAX_STEPS)
        .collect();

    Ok(AiDiagnosis {
        summary: if summary.is_empty() {
            "模型未给出结论".to_string()
        } else {
            summary
        },
        likely_cause: if likely_cause.is_empty() {
            "模型未给出原因分析".to_string()
        } else {
            likely_cause
        },
        confidence: parse_confidence(wire.confidence.as_deref()),
        steps,
        actions,
        log_bytes,
        credential_suspect,
        credential_note,
        model: model.to_string(),
        analyzed_at: now_string(),
    })
}

fn parse_confidence(value: Option<&str>) -> AiConfidence {
    match value.unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "high" => AiConfidence::High,
        "low" => AiConfidence::Low,
        _ => AiConfidence::Medium,
    }
}

fn parse_command(value: &str) -> Option<DiagnosticCommandId> {
    match value.trim() {
        "diskUsage" | "disk_usage" => Some(DiagnosticCommandId::DiskUsage),
        "memory" => Some(DiagnosticCommandId::Memory),
        "failedServices" | "failed_services" => Some(DiagnosticCommandId::FailedServices),
        "targetDir" | "target_dir" => Some(DiagnosticCommandId::TargetDir),
        "docker" => Some(DiagnosticCommandId::Docker),
        _ => None,
    }
}

fn clip(value: &str, max_chars: usize) -> String {
    value.trim().chars().take(max_chars).collect()
}

/// 预览：与 `analyze` 共用 `build_context`，保证「看到的」就是「发出去的」。
pub fn preview(store: &Store, record_id: &str) -> Result<DiagnosisPreview> {
    let record = store.find_record(record_id)?;
    let (context, log_bytes) = build_context(&record);
    Ok(DiagnosisPreview {
        total_bytes: context.len(),
        context,
        log_bytes,
    })
}

/// 分析一条失败的部署记录，并把结果写回记录（写回失败不影响本次结果返回）。
pub async fn analyze(store: &Store, record_id: &str) -> Result<AiDiagnosis> {
    let config = store.load_config()?;
    let base_url = config.settings.ai_base_url.trim().to_string();
    let api_key = config.settings.ai_api_key.trim().to_string();
    let model = config.settings.ai_model.trim().to_string();
    if base_url.is_empty() || api_key.is_empty() || model.is_empty() {
        return Err(CoreError::config(
            "尚未配置 AI 诊断：请在「设置 → 云端凭据」填写 Base URL、API Key 与模型名",
        ));
    }
    // Base URL 只放行 http(s)：配置可以被导入，不能让它把请求指到 file:// 或别的协议上。
    let lower_base = base_url.to_ascii_lowercase();
    if !(lower_base.starts_with("http://") || lower_base.starts_with("https://")) {
        return Err(CoreError::config(
            "AI 诊断的 Base URL 必须以 http:// 或 https:// 开头",
        ));
    }
    // Key 会写进请求头文件，换行等于让调用方多塞一条 header。
    if api_key.contains('\r') || api_key.contains('\n') {
        return Err(CoreError::config("AI 诊断的 API Key 不能包含换行"));
    }
    let record = store.find_record(record_id)?;
    if record.status != DeployStatus::Failed {
        return Err(CoreError::diagnosis("只有失败的部署记录才能做 AI 诊断"));
    }

    let (context, log_bytes) = build_context(&record);
    let request = build_request_json(&model, &context);
    let content = call_model(&base_url, &api_key, &request).await?;
    let diagnosis = parse_diagnosis(&content, &model, log_bytes)?;

    if let Ok(mut saved) = store.find_record(record_id) {
        saved.ai_diagnosis = Some(diagnosis.clone());
        let limit = store
            .load_config()
            .map(|config| config.settings.history_limit)
            .unwrap_or(500);
        let _ = store.upsert_history(&saved, limit);
    }
    Ok(diagnosis)
}

/// 执行一条白名单内的只读诊断命令。命令本身由后端拼装，模型只负责挑选 id。
pub async fn run_diagnostic(
    store: &Store,
    record_id: &str,
    command: DiagnosticCommandId,
) -> Result<DiagnosticOutput> {
    let config = store.load_config()?;
    let record = store.find_record(record_id)?;
    let server = Store::find_server(&config, &record.server_id)?.clone();
    let script = diagnostic_script(command, &record.target_dir);
    let client = SshClient::connect(&server, config.settings.connect_timeout_secs).await?;
    let result = client.exec_capture(&script, DIAGNOSTIC_TIMEOUT_SECS).await;
    client.disconnect().await;
    let (code, output) = result?;
    let truncated = output.len() > MAX_DIAGNOSTIC_BYTES;
    let (output, _) = tail(&output, MAX_DIAGNOSTIC_BYTES);
    Ok(DiagnosticOutput {
        command,
        code,
        output: output.to_string(),
        truncated,
    })
}

fn diagnostic_script(command: DiagnosticCommandId, target_dir: &str) -> String {
    match command {
        DiagnosticCommandId::DiskUsage => {
            "df -h / 2>&1 | head -n 20; echo '--- inode ---'; df -i / 2>&1 | head -n 20".to_string()
        }
        DiagnosticCommandId::Memory => "free -m 2>&1 | head -n 5".to_string(),
        DiagnosticCommandId::FailedServices => {
            "systemctl --failed --no-pager 2>&1 | head -n 40".to_string()
        }
        DiagnosticCommandId::TargetDir => {
            format!("ls -la {} 2>&1 | head -n 80", shell_quote(target_dir))
        }
        DiagnosticCommandId::Docker => {
            "docker ps --format 'table {{.Names}}\\t{{.Status}}\\t{{.Image}}' 2>&1 | head -n 40"
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DeployRecord, DeployStatus};

    fn record(log: &str) -> DeployRecord {
        DeployRecord {
            id: "r1".to_string(),
            repo_id: "repo".to_string(),
            repo_name: "demo".to_string(),
            rev: "main".to_string(),
            branch: "main".to_string(),
            worktree: false,
            atomic_release: false,
            release_dir: None,
            commit: "abcdef".to_string(),
            commit_short: "abcdef".to_string(),
            commit_subject: "init".to_string(),
            server_id: "srv".to_string(),
            server_name: "prod".to_string(),
            target_dir: "/srv/app".to_string(),
            script_dir: "docker".to_string(),
            scripts: vec!["deploy.sh".to_string()],
            run_scripts: true,
            env_files: vec![crate::models::EnvFileConfig {
                local_path: "/home/me/.env.secret".to_string(),
                remote_path: ".env".to_string(),
            }],
            status: DeployStatus::Failed,
            error: Some("script failed".to_string()),
            log: log.to_string(),
            started_at: "2026-01-01 00:00:00".to_string(),
            finished_at: Some("2026-01-01 00:01:00".to_string()),
            duration_ms: 60_000,
            ai_diagnosis: None,
        }
    }

    #[test]
    fn masks_password_token_and_bearer_values() {
        let text = "PASSWORD=hunter2\napi_key: sk-123\nexport TOKEN=\"abc def\"\nAuthorization: Bearer eyJhbGciOi\nDB_PASSWORD: 'p@ss'\nexport AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI\nexport AWS_ACCESS_KEY_ID=AKIA123";
        let masked = redact_for_llm(text);
        for secret in [
            "hunter2",
            "sk-123",
            "abc def",
            "eyJhbGciOi",
            "p@ss",
            "wJalrXUtnFEMI",
            "AKIA123",
        ] {
            assert!(!masked.contains(secret), "泄露了 {secret}: {masked}");
        }
        assert!(masked.contains("PASSWORD=***"));
    }

    #[test]
    fn keeps_text_without_assignment_untouched() {
        let text = "token 已过期，请重新登录\nbuild failed after 3m";
        assert_eq!(redact_for_llm(text), text);
    }

    #[test]
    fn strips_private_key_blocks_and_ansi() {
        let text = "\u{1b}[31mred\u{1b}[0m\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\ndone";
        let masked = redact_for_llm(text);
        assert!(masked.contains("red"));
        assert!(!masked.contains("AAAA"));
        assert!(masked.contains("[已隐去私钥内容]"));
        assert!(!masked.contains('\u{1b}'));
    }

    #[test]
    fn masks_remote_url_credentials() {
        let text = "fatal: unable to access 'https://user:ghp_secret@github.com/a/b.git'";
        let masked = redact_for_llm(text);
        assert!(!masked.contains("ghp_secret"), "{masked}");
        assert!(masked.contains("github.com"));
    }

    #[test]
    fn context_never_contains_env_paths_or_secrets() {
        let record = record("PASSWORD=hunter2\nscript failed");
        let (context, log_bytes) = build_context(&record);
        assert!(!context.contains("hunter2"));
        assert!(!context.contains(".env.secret"), "env 文件路径不该发给模型");
        assert_eq!(log_bytes, redact_for_llm(&record.log).len());
        assert!(context.contains("deploy.sh"));
    }

    #[test]
    fn tail_keeps_char_boundary() {
        let text = "一二三四五六七八九十";
        let (tail, truncated) = tail(text, 7);
        assert!(truncated);
        assert!(text.ends_with(tail));
        assert!(std::str::from_utf8(tail.as_bytes()).is_ok());
    }

    #[test]
    fn actions_are_whitelisted_and_credential_clears_them() {
        let content = r#"{"summary":"构建失败","likelyCause":"依赖缺失","confidence":"high","steps":["补依赖"],"actions":[{"type":"redeploy"},{"type":"rm_rf"},{"type":"runDiagnostic","command":"diskUsage"},{"type":"runDiagnostic","command":"rm -rf /"}],"credentialSuspect":false,"credentialNote":null}"#;
        let diagnosis = parse_diagnosis(content, "m", 10).unwrap();
        assert_eq!(
            diagnosis.actions,
            vec![
                AiFixAction::Redeploy,
                AiFixAction::RunDiagnostic {
                    command: DiagnosticCommandId::DiskUsage
                }
            ]
        );

        let credential = content.replace("\"credentialSuspect\":false", "\"credentialSuspect\":true");
        let diagnosis = parse_diagnosis(&credential, "m", 10).unwrap();
        assert!(diagnosis.credential_suspect);
        assert!(diagnosis.actions.is_empty());
        assert!(diagnosis.credential_note.unwrap().contains("凭据"));
    }

    #[test]
    fn parses_json_inside_code_fence() {
        let content = "```json\n{\"summary\":\"s\",\"likelyCause\":\"c\",\"confidence\":\"low\",\"steps\":[],\"actions\":[],\"credentialSuspect\":false,\"credentialNote\":null}\n```";
        let diagnosis = parse_diagnosis(content, "m", 0).unwrap();
        assert_eq!(diagnosis.confidence, AiConfidence::Low);
    }

    #[test]
    fn target_dir_is_shell_quoted() {
        let script = diagnostic_script(DiagnosticCommandId::TargetDir, "/srv/my app; rm -rf /");
        assert!(script.contains("'/srv/my app; rm -rf /'"), "{script}");
    }

    /// 这几个字段名 / 枚举字面量是前端 `src/lib/types/deploy.ts` 的手工镜像，改一侧必须同步。
    #[test]
    fn json_shapes_match_the_frontend_types() {
        let action = AiFixAction::RunDiagnostic {
            command: DiagnosticCommandId::DiskUsage,
        };
        assert_eq!(
            serde_json::to_string(&action).unwrap(),
            r#"{"type":"runDiagnostic","command":"diskUsage"}"#
        );
        assert_eq!(
            serde_json::to_string(&AiFixAction::OpenTerminal).unwrap(),
            r#"{"type":"openTerminal"}"#
        );

        let diagnosis = AiDiagnosis {
            summary: "s".to_string(),
            likely_cause: "c".to_string(),
            confidence: AiConfidence::High,
            steps: vec![],
            actions: vec![],
            log_bytes: 1,
            credential_suspect: false,
            credential_note: None,
            model: "m".to_string(),
            analyzed_at: "t".to_string(),
        };
        let value = serde_json::to_value(&diagnosis).unwrap();
        for key in [
            "summary",
            "likelyCause",
            "confidence",
            "steps",
            "actions",
            "logBytes",
            "credentialSuspect",
            "credentialNote",
            "model",
            "analyzedAt",
        ] {
            assert!(value.get(key).is_some(), "缺少字段 {key}: {value}");
        }
        assert_eq!(value["confidence"], "high");
        assert!(value["credentialNote"].is_null());
    }
}
