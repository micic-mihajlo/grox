//! Codex subscription connector backed by `codex app-server`.

use crate::app::cli::CodexArgs;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write as _;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
struct CodexStatus {
    account_label: String,
    models: Vec<String>,
    default_model: Option<String>,
}

struct AppServerConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    notifications: VecDeque<Value>,
}

impl AppServerConnection {
    async fn start(binary: &std::path::Path) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .arg("app-server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().with_context(|| {
            format!(
                "could not start Codex CLI at '{}'; install @openai/codex and run `codex login`",
                binary.display()
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .context("Codex app-server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("Codex app-server stdout unavailable")?;
        if let Some(mut stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            });
        }

        let mut connection = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
            notifications: VecDeque::new(),
        };
        connection
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "grox_cli",
                        "title": "Grox",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": { "experimentalApi": true }
                }),
            )
            .await?;
        connection.notify("initialized", json!({})).await?;
        Ok(connection)
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.write_json(&json!({ "id": id, "method": method, "params": params }))
            .await?;

        tokio::time::timeout(REQUEST_TIMEOUT, async {
            loop {
                let message = self.read_message().await?;
                if message.get("id").and_then(Value::as_u64) == Some(id)
                    && message.get("method").is_none()
                {
                    if let Some(error) = message.get("error") {
                        bail!("{}", format_rpc_error(error));
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                if is_server_request(&message) {
                    self.answer_server_request(&message).await?;
                } else {
                    self.notifications.push_back(message);
                }
            }
        })
        .await
        .map_err(|_| anyhow!("Codex app-server request timed out: {method}"))?
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_json(&json!({ "method": method, "params": params }))
            .await
    }

    async fn next_notification(&mut self) -> Result<Value> {
        loop {
            if let Some(message) = self.notifications.pop_front() {
                return Ok(message);
            }
            let message = self.read_message().await?;
            if is_server_request(&message) {
                self.answer_server_request(&message).await?;
                continue;
            }
            if message.get("method").is_some() {
                return Ok(message);
            }
        }
    }

    async fn read_message(&mut self) -> Result<Value> {
        loop {
            let line = self
                .stdout
                .next_line()
                .await
                .context("failed reading Codex app-server output")?
                .ok_or_else(|| anyhow!("Codex app-server exited unexpectedly"))?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(message) = serde_json::from_str(&line) {
                return Ok(message);
            }
        }
    }

    async fn write_json(&mut self, value: &Value) -> Result<()> {
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        self.stdin
            .write_all(&encoded)
            .await
            .context("failed writing to Codex app-server")?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn answer_server_request(&mut self, request: &Value) -> Result<()> {
        let Some(id) = request.get("id").cloned() else {
            return Ok(());
        };
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                json!({ "decision": "decline" })
            }
            "item/tool/requestUserInput" => {
                let mut answers = serde_json::Map::new();
                for question in request
                    .pointer("/params/questions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(question_id) = question.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    let first = question
                        .get("options")
                        .and_then(Value::as_array)
                        .and_then(|options| options.first())
                        .and_then(|option| option.get("label"))
                        .and_then(Value::as_str);
                    answers.insert(
                        question_id.to_owned(),
                        json!({ "answers": first.into_iter().collect::<Vec<_>>() }),
                    );
                }
                json!({ "answers": answers })
            }
            _ => {
                self.write_json(&json!({
                    "id": id,
                    "error": { "code": -32601, "message": format!("unsupported server request: {method}") }
                }))
                .await?;
                return Ok(());
            }
        };
        self.write_json(&json!({ "id": id, "result": result }))
            .await
    }

    async fn status(&mut self) -> Result<CodexStatus> {
        let account = self.request("account/read", json!({})).await?;
        let account = account
            .get("account")
            .filter(|value| !value.is_null())
            .ok_or_else(|| {
                anyhow!("Codex CLI is not authenticated; run `codex login` and try again")
            })?;
        let account_label = match account.get("type").and_then(Value::as_str) {
            Some("chatgpt") => format!(
                "ChatGPT {} subscription",
                account
                    .get("planType")
                    .and_then(Value::as_str)
                    .unwrap_or("authenticated")
            ),
            Some("apiKey") => "OpenAI API key".to_owned(),
            Some("amazonBedrock") => "Amazon Bedrock".to_owned(),
            Some(other) => other.to_owned(),
            None => "authenticated Codex account".to_owned(),
        };

        let mut models = Vec::new();
        let mut default_model = None;
        let mut cursor: Option<String> = None;
        loop {
            let response = self
                .request(
                    "model/list",
                    cursor
                        .as_ref()
                        .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor })),
                )
                .await?;
            for model in response
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(id) = model.get("model").and_then(Value::as_str) else {
                    continue;
                };
                if model.get("isDefault").and_then(Value::as_bool) == Some(true) {
                    default_model = Some(id.to_owned());
                }
                models.push(id.to_owned());
            }
            cursor = response
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        if default_model.is_none() {
            default_model = models.first().cloned();
        }
        Ok(CodexStatus {
            account_label,
            models,
            default_model,
        })
    }

    async fn open_thread(
        &mut self,
        cwd: &std::path::Path,
        model: Option<&str>,
        resume: Option<&str>,
        full_access: bool,
    ) -> Result<String> {
        let sandbox = if full_access {
            "danger-full-access"
        } else {
            "workspace-write"
        };
        let mut params = json!({
            "cwd": cwd,
            "approvalPolicy": "never",
            "sandbox": sandbox
        });
        if let Some(model) = model {
            params["model"] = json!(model);
        }

        let response = if let Some(thread_id) = resume {
            let mut resume_params = params.clone();
            resume_params["threadId"] = json!(thread_id);
            self.request("thread/resume", resume_params)
                .await
                .with_context(|| format!("could not resume Codex thread '{thread_id}'"))?
        } else {
            self.request("thread/start", params).await?
        };
        response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("Codex app-server did not return a thread id"))
    }

    async fn run_turn(
        &mut self,
        thread_id: &str,
        prompt: &str,
        model: Option<&str>,
        cwd: &std::path::Path,
        full_access: bool,
    ) -> Result<()> {
        let sandbox = if full_access {
            json!({ "type": "dangerFullAccess" })
        } else {
            json!({
                "type": "workspaceWrite",
                "writableRoots": [cwd],
                "networkAccess": false,
                "excludeTmpdirEnvVar": false,
                "excludeSlashTmp": false
            })
        };
        let mut params = json!({
            "threadId": thread_id,
            "input": [{ "type": "text", "text": prompt }],
            "approvalPolicy": "never",
            "sandboxPolicy": sandbox
        });
        if let Some(model) = model {
            params["model"] = json!(model);
        }
        let response = self.request("turn/start", params).await?;
        let turn_id = response
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let mut wrote_text = false;

        loop {
            let notification = self.next_notification().await?;
            if notification
                .pointer("/params/threadId")
                .and_then(Value::as_str)
                .is_some_and(|id| id != thread_id)
            {
                continue;
            }
            match notification.get("method").and_then(Value::as_str) {
                Some("item/agentMessage/delta" | "item/plan/delta") => {
                    if let Some(delta) = notification
                        .pointer("/params/delta")
                        .and_then(Value::as_str)
                    {
                        print!("{delta}");
                        std::io::stdout().flush()?;
                        wrote_text = true;
                    }
                }
                Some("item/started") => {
                    if let Some(label) = tool_label(notification.pointer("/params/item")) {
                        eprintln!("\x1b[2m[{label}]\x1b[0m");
                    }
                }
                Some("error")
                    if notification.pointer("/params/willRetry") != Some(&Value::Bool(true)) =>
                {
                    let message = notification
                        .pointer("/params/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex app-server reported an error");
                    bail!("{message}");
                }
                Some("turn/completed") => {
                    if let Some(expected) = turn_id.as_deref()
                        && notification
                            .pointer("/params/turn/id")
                            .and_then(Value::as_str)
                            != Some(expected)
                    {
                        continue;
                    }
                    let status = notification
                        .pointer("/params/turn/status")
                        .and_then(Value::as_str)
                        .unwrap_or("completed");
                    if status == "failed" {
                        let message = notification
                            .pointer("/params/turn/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("Codex turn failed");
                        bail!("{message}");
                    }
                    break;
                }
                _ => {}
            }
        }
        if wrote_text {
            println!();
        }
        Ok(())
    }

    async fn close(mut self) {
        let _ = self.child.kill().await;
    }
}

/// Run the `grox codex` connector.
pub async fn run(args: CodexArgs) -> Result<()> {
    let mut connection = AppServerConnection::start(&args.codex_binary).await?;
    let status = connection.status().await?;
    if args.status {
        println!("Codex authentication: {}", status.account_label);
        println!(
            "Default model: {}",
            status
                .default_model
                .as_deref()
                .unwrap_or("Codex configuration default")
        );
        println!("Available models:");
        for model in &status.models {
            println!("  {model}");
        }
        connection.close().await;
        return Ok(());
    }

    let cwd = std::env::current_dir()?;
    let mut model = args.model.or(status.default_model);
    let thread_id = connection
        .open_thread(
            &cwd,
            model.as_deref(),
            args.resume.as_deref(),
            args.full_access,
        )
        .await?;
    let initial_prompt = args.prompt.or(args.message);
    if let Some(prompt) = initial_prompt {
        connection
            .run_turn(
                &thread_id,
                &prompt,
                model.as_deref(),
                &cwd,
                args.full_access,
            )
            .await?;
        eprintln!("\x1b[2mCodex thread: {thread_id}\x1b[0m");
        connection.close().await;
        return Ok(());
    }

    eprintln!(
        "Grox Codex • {} • model {}",
        status.account_label,
        model.as_deref().unwrap_or("default")
    );
    eprintln!("Thread {thread_id} • /help for commands");
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    loop {
        eprint!("\n\x1b[1;36m›\x1b[0m ");
        std::io::stderr().flush()?;
        let Some(line) = lines.next_line().await? else {
            break;
        };
        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        match prompt {
            "/exit" | "/quit" => break,
            "/help" => {
                eprintln!("/model <id>  switch model for following turns");
                eprintln!("/models      list subscription models");
                eprintln!("/thread      print the resumable Codex thread id");
                eprintln!("/exit        quit");
            }
            "/models" => {
                for available in &status.models {
                    eprintln!("{available}");
                }
            }
            "/thread" => eprintln!("{thread_id}"),
            _ if prompt.starts_with("/model ") => {
                let requested = prompt.trim_start_matches("/model ").trim();
                if requested.is_empty() {
                    eprintln!("usage: /model <id>");
                } else {
                    model = Some(requested.to_owned());
                    eprintln!("model: {requested}");
                }
            }
            _ => {
                if let Err(error) = connection
                    .run_turn(&thread_id, prompt, model.as_deref(), &cwd, args.full_access)
                    .await
                {
                    eprintln!("Codex error: {error:#}");
                }
            }
        }
    }
    connection.close().await;
    Ok(())
}

fn is_server_request(message: &Value) -> bool {
    message.get("id").is_some() && message.get("method").is_some()
}

fn format_rpc_error(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("Codex app-server request failed");
    match error.get("code").and_then(Value::as_i64) {
        Some(code) => format!("{message} ({code})"),
        None => message.to_owned(),
    }
}

fn tool_label(item: Option<&Value>) -> Option<String> {
    let item = item?;
    match item.get("type").and_then(Value::as_str)? {
        "commandExecution" => Some(format!(
            "shell: {}",
            item.get("command")
                .and_then(Value::as_str)
                .unwrap_or("command")
        )),
        "fileChange" => Some("apply patch".to_owned()),
        "mcpToolCall" => Some(format!(
            "{}.{}",
            item.get("server").and_then(Value::as_str).unwrap_or("mcp"),
            item.get("tool").and_then(Value::as_str).unwrap_or("tool")
        )),
        "webSearch" => Some("web search".to_owned()),
        "collabAgentToolCall" => Some("collaboration agent".to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_rpc_errors_with_codes() {
        assert_eq!(
            format_rpc_error(&json!({ "code": -32000, "message": "no thread" })),
            "no thread (-32000)"
        );
    }

    #[test]
    fn labels_command_and_mcp_activity() {
        assert_eq!(
            tool_label(Some(
                &json!({ "type": "commandExecution", "command": "cargo test" })
            )),
            Some("shell: cargo test".to_owned())
        );
        assert_eq!(
            tool_label(Some(
                &json!({ "type": "mcpToolCall", "server": "docs", "tool": "search" })
            )),
            Some("docs.search".to_owned())
        );
    }

    #[test]
    fn distinguishes_server_requests_from_notifications() {
        assert!(is_server_request(&json!({ "id": 4, "method": "request" })));
        assert!(!is_server_request(&json!({ "method": "turn/completed" })));
    }
}
