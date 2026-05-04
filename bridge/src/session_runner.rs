//! Session 子进程管理

use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{debug, error, info};
use types::bridge::{SessionActivity, SessionActivityType, SessionDoneStatus};

#[derive(Clone, Debug)]
pub struct SessionSpawnConfig {
    pub executable: PathBuf,
    pub work_dir: PathBuf,
    pub session_id: String,
    pub sdk_url: String,
    pub access_token: String,
    pub extra_env: Vec<(String, String)>,
    pub verbose: bool,
    pub sandbox: bool,
}

pub struct SessionHandle {
    pub session_id: String,
    pub activity_rx: mpsc::Receiver<SessionActivity>,
    pub done_rx: tokio::sync::oneshot::Receiver<SessionDoneStatus>,
    // 保存 kill handle，不需要直接持有 Child
    kill_tx: mpsc::Sender<bool>, // true = force kill
}

impl SessionHandle {
    /// 请求优雅退出
    pub fn kill(&self) {
        let _ = self.kill_tx.try_send(false);
    }

    /// 强制退出
    pub fn force_kill(&self) {
        let _ = self.kill_tx.try_send(true);
    }
}

pub async fn spawn_session(config: SessionSpawnConfig) -> std::io::Result<SessionHandle> {
    let (activity_tx, activity_rx) = mpsc::channel(64);
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let (kill_tx, mut kill_rx) = mpsc::channel::<bool>(1);

    let mut cmd = Command::new(&config.executable);
    cmd.current_dir(&config.work_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .arg("--session-mode")
        .arg("--session-id")
        .arg(&config.session_id)
        .arg("--sdk-url")
        .arg(&config.sdk_url)
        .env("MARGATROID_SESSION_ACCESS_TOKEN", &config.access_token)
        .env("MARGATROID_SESSION_ID", &config.session_id);

    if config.verbose {
        cmd.arg("--verbose");
    }
    if config.sandbox {
        cmd.env("MARGATROID_FORCE_SANDBOX", "1");
    }
    for (k, v) in &config.extra_env {
        cmd.env(k, v);
    }
    cmd.env_remove("MARGATROID_OAUTH_TOKEN");

    let mut child: Child = cmd.spawn()?;
    let session_id = config.session_id.clone();

    // 读取 stdout
    if let Some(stdout) = child.stdout.take() {
        let tx = activity_tx.clone();
        let sid = session_id.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!("session {sid} stdout: {line}");
                if let Some(activity) = parse_activity_line(&line) {
                    let _ = tx.send(activity).await;
                }
            }
        });
    }

    // 读取 stderr
    if let Some(stderr) = child.stderr.take() {
        let sid = session_id.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                info!("session {sid} stderr: {line}");
            }
        });
    }

    // 监控进程：等待退出或收到 kill 信号
    let session_id_monitor = session_id.clone();
    tokio::spawn(async move {
        let session_id = session_id_monitor;
        tokio::select! {
            // 进程自然退出
            status = child.wait() => {
                match status {
                    Ok(s) if s.success() => {
                        info!("session {session_id} completed successfully");
                        let _ = done_tx.send(SessionDoneStatus::Completed);
                    }
                    Ok(_) => {
                        info!("session {session_id} failed");
                        let _ = done_tx.send(SessionDoneStatus::Failed);
                    }
                    Err(e) => {
                        error!("session {session_id} wait error: {e}");
                        let _ = done_tx.send(SessionDoneStatus::Failed);
                    }
                }
            }
            // 收到 kill 信号
            Some(force) = kill_rx.recv() => {
                if force {
                    if let Err(e) = child.kill().await {
                        error!("force kill session {session_id}: {e}");
                    }
                } else {
                    // 发送 SIGTERM（tokio 在 Unix 上使用 SIGTERM，Windows 上使用 TerminateProcess）
                    if let Err(e) = child.start_kill() {
                        error!("kill session {session_id}: {e}");
                    }
                    // 等待进程退出，最多 5 秒后强制杀
                    tokio::select! {
                        _ = child.wait() => {}
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                            let _ = child.kill().await;
                        }
                    }
                }
                let _ = done_tx.send(SessionDoneStatus::Interrupted);
            }
        }
    });

    Ok(SessionHandle {
        session_id,
        activity_rx,
        done_rx,
        kill_tx,
    })
}

fn parse_activity_line(line: &str) -> Option<SessionActivity> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind_str = v.get("type")?.as_str()?;
    let summary = v
        .get("summary")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    let kind = match kind_str {
        "tool_start" => SessionActivityType::ToolStart,
        "text" => SessionActivityType::Text,
        "result" => SessionActivityType::Result,
        "error" => SessionActivityType::Error,
        _ => return None,
    };

    Some(SessionActivity {
        kind,
        summary,
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    })
}
