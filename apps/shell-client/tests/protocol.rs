#![cfg(unix)]
use serde_json::{json, Value};
use std::{io, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Client {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_shell-client"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }
    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        self.input.write_all(&bytes).await.unwrap();
    }
    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        let n = tokio::time::timeout(Duration::from_secs(10), self.output.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert!(n > 0, "unexpected protocol EOF");
        serde_json::from_str(&line).unwrap()
    }
    async fn success(&mut self) {
        assert!(
            tokio::time::timeout(Duration::from_secs(10), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}

#[tokio::test]
async fn real_pipes_keep_sh_syntax_stderr_and_exit_status() {
    let mut client = Client::new();
    client.send(json!({"operation":"exec", "command":"printf '%s' '$literal'; printf err >&2; exit 7", "max_output_bytes":1024})).await;
    assert_eq!(
        client.recv().await,
        json!({"type":"result","stdout":"$literal","stderr":"err","code":7,"exit_code":7,"signal":null,"truncated":false,"terminal":false})
    );
    client.success().await;
}

#[tokio::test]
async fn bash_script_is_passed_through_sh_unchanged() {
    let mut client = Client::new();
    client.send(json!({"operation":"exec", "command":"bash --noprofile --norc -c 'a=(one two); printf \"%s\" \"${a[1]}\"'", "max_output_bytes":1024})).await;
    let result = client.recv().await;
    assert_eq!(result["stdout"], "two");
    assert_eq!(result["exit_code"], 0);
    client.success().await;
}

#[tokio::test]
async fn excessive_output_is_drained_after_truncation() {
    let mut client = Client::new();
    client.send(json!({"operation":"exec", "command":"head -c 262144 /dev/zero; head -c 262144 /dev/zero >&2; exit 9", "max_output_bytes":17})).await;
    let result = client.recv().await;
    assert_eq!(result["stdout"].as_str().unwrap().len(), 17);
    assert_eq!(result["stderr"].as_str().unwrap().len(), 17);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["exit_code"], 9);
    client.success().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn once_exec_pty_is_a_real_terminal() {
    let mut client = Client::new();
    client.send(json!({"operation":"exec", "command":"test -t 0 && test -t 1 && test -t 2 && printf 'tty-ok\\n'", "pty":true, "max_output_bytes":1024})).await;
    let result = client.recv().await;
    assert_eq!(result["stdout"], "tty-ok\r\n");
    assert_eq!(result["stderr"], "");
    assert_eq!(result["terminal"], true);
    assert_eq!(result["exit_code"], 0);
    client.success().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn interactive_pty_accepts_input_and_preserves_eighty_lines() {
    let mut client = Client::new();
    client
        .send(json!({"operation":"terminal", "max_output_bytes":32768}))
        .await;
    assert_eq!(client.recv().await, json!({"type":"ready"}));
    // Disable echo to keep the command source out of the transcript.
    client.send(json!({"operation":"write", "text":"stty -echo; PS1=''; printf '\\033[31m'; for i in {1..80}; do printf 'ROW_%03d\\n' \"$i\"; done; printf '\\033[0mDONE_MARK\\n'; exit 6\r"})).await;
    let mut transcript = String::new();
    loop {
        let event = client.recv().await;
        match event["type"].as_str().unwrap() {
            "output" => {
                if event["replace"] == true {
                    transcript.clear();
                }
                transcript.push_str(event["text"].as_str().unwrap());
            }
            "exit" => {
                assert_eq!(event["exit_code"], 6);
                break;
            }
            _ => panic!("unexpected frame: {event}"),
        }
    }
    for i in 1..=80 {
        assert!(
            transcript.contains(&format!("ROW_{i:03}")),
            "missing row {i}: {transcript}"
        );
    }
    assert!(transcript.contains("DONE_MARK"));
    assert!(!transcript.contains('\x1b'));
    client.success().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn terminal_budget_emits_error_and_nonzero_exit() {
    let mut client = Client::new();
    client
        .send(json!({"operation":"terminal", "max_output_bytes":1}))
        .await;
    assert_eq!(client.recv().await["type"], "ready");
    assert_eq!(client.recv().await["type"], "error");
    assert!(
        !tokio::time::timeout(Duration::from_secs(10), client.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn terminal_exits_while_disowned_child_still_holds_slave() {
    let mut client = Client::new();
    client
        .send(json!({"operation":"terminal", "max_output_bytes":32768}))
        .await;
    assert_eq!(client.recv().await["type"], "ready");
    client.send(json!({"operation":"write", "text":"sleep 30 & printf '\\nDETACHED_PID=%d\\n' \"$!\"; disown; printf 'TAIL_BEFORE_EXIT\\n'; exit 7\r"})).await;
    let mut transcript = String::new();
    loop {
        let event = client.recv().await;
        if event["type"] == "exit" {
            assert_eq!(event["exit_code"], 7);
            break;
        }
        assert_eq!(event["type"], "output");
        if event["replace"] == true {
            transcript.clear();
        }
        transcript.push_str(event["text"].as_str().unwrap());
    }
    let pid = transcript
        .lines()
        .find_map(|line| {
            line.strip_prefix("DETACHED_PID=")
                .and_then(|value| value.parse::<i32>().ok())
        })
        .expect("detached child PID in terminal output");
    let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
    assert!(
        alive,
        "normal terminal completion killed its disowned child"
    );
    assert!(transcript.lines().any(|line| line == "TAIL_BEFORE_EXIT"));
    client.success().await;
    let mut tail = String::new();
    assert_eq!(
        client.output.read_line(&mut tail).await.unwrap(),
        0,
        "exit must be the final frame"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn terminal_close_does_not_wait_behind_child_wait() {
    let mut client = Client::new();
    client
        .send(json!({"operation":"terminal", "max_output_bytes":32768}))
        .await;
    assert_eq!(client.recv().await["type"], "ready");
    client
        .send(json!({"operation":"write", "text":"sleep 1000\r"}))
        .await;
    client.send(json!({"operation":"close"})).await;
    loop {
        let event = client.recv().await;
        if event["type"] == "exit" {
            break;
        }
        assert_eq!(event["type"], "output");
    }
    client.success().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn close_progresses_when_protocol_output_is_backpressured() {
    let mut client = Client::new();
    client
        .send(json!({"operation":"terminal", "max_output_bytes":65536}))
        .await;
    assert_eq!(client.recv().await["type"], "ready");
    client.send(json!({"operation":"write", "text":"stty -echo; i=0; while :; do printf '\\033[H%01900d' \"$((i++))\"; done\r"})).await;
    assert_eq!(client.recv().await["type"], "output");
    // Let stdout and its bounded frame queue fill while no consumer reads.
    tokio::time::sleep(Duration::from_millis(100)).await;
    client.send(json!({"operation":"close"})).await;
    loop {
        let event = client.recv().await;
        if event["type"] == "exit" {
            break;
        }
        assert_eq!(event["type"], "output");
    }
    client.success().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn real_alternate_screen_replaces_then_restores_transcript() {
    let mut client = Client::new();
    client
        .send(json!({"operation":"terminal", "max_output_bytes":32768}))
        .await;
    assert_eq!(client.recv().await["type"], "ready");
    client.send(json!({"operation":"write", "text":"stty -echo; PS1=''; printf '\\033[?1049h\\033[2J\\033[HSCREEN_ONE'\r"})).await;
    loop {
        let event = client.recv().await;
        assert_eq!(event["type"], "output");
        if event["replace"] == true && event["text"].as_str().unwrap().contains("SCREEN_ONE") {
            break;
        }
    }
    client
        .send(json!({"operation":"write", "text":"printf '\\033[HSCREEN_TWO\\033[K'\r"}))
        .await;
    loop {
        let event = client.recv().await;
        assert_eq!(event["type"], "output");
        if event["replace"] == true && event["text"].as_str().unwrap().contains("SCREEN_TWO") {
            break;
        }
    }
    client
        .send(json!({"operation":"write", "text":"printf '\\033[?1049l'; exit\r"}))
        .await;
    let mut restored = false;
    loop {
        let event = client.recv().await;
        if event["type"] == "exit" {
            break;
        }
        assert_eq!(event["type"], "output");
        restored |=
            event["replace"] == true && !event["text"].as_str().unwrap().contains("SCREEN_TWO");
    }
    assert!(restored);
    client.success().await;
}

#[tokio::test]
async fn malformed_initial_request_emits_error() {
    let mut client = Client::new();
    client.send(json!({"operation":"write", "text":"no"})).await;
    assert_eq!(client.recv().await["type"], "error");
    assert!(!client.child.wait().await.unwrap().success());
}

#[tokio::test]
async fn parent_eof_cleans_up_child_group() {
    let mut client = Client::new();
    client
        .send(json!({"operation":"exec", "command":"sleep 1000 & wait", "max_output_bytes":1024}))
        .await;
    drop(client.input);
    assert!(
        tokio::time::timeout(Duration::from_secs(10), client.child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn normal_completion_preserves_detached_child() -> io::Result<()> {
    let mut client = Client::new();
    client.send(json!({"operation":"exec", "command":"sleep 1000 </dev/null >/dev/null 2>&1 & printf '%s' $!", "max_output_bytes":1024})).await;
    let result = client.recv().await;
    let pid: i32 = result["stdout"].as_str().unwrap().parse().unwrap();
    client.success().await;
    let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
    assert!(alive, "normal exec killed a detached descendant");
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn terminal_tracks_real_stty_resizes_and_preserves_tui_rows() {
    let mut client = Client::new();
    client
        .send(json!({"operation":"terminal","max_output_bytes":32768}))
        .await;
    assert_eq!(client.recv().await["type"], "ready");
    client.send(json!({"operation":"write","text":concat!(r"stty -echo; PS1=''; stty rows 40 cols 120; printf '\033[?1049h\033[2J\033[HWIDE\033[39;1HLOWER\033[40;120HZ'; read -r resize_continue; stty rows 10 cols 30; printf '\033[2J\033[HSMALL\033[10;30HQ'; read -r exit_continue; printf '\033[?1049l'; exit", "\r")})).await;
    loop {
        let value = client.recv().await;
        assert_eq!(value["type"], "output", "{value}");
        let text = value["text"].as_str().unwrap();
        if text.starts_with("WIDE") && text.ends_with('Z') {
            assert_eq!(value["replace"], true);
            let rows = text.split('\n').collect::<Vec<_>>();
            assert_eq!(rows.len(), 40);
            assert_eq!(rows[38], "LOWER");
            assert_eq!(rows[39], format!("{}Z", " ".repeat(119)));
            break;
        }
    }
    client
        .send(json!({"operation":"write","text":"continue\r"}))
        .await;
    loop {
        let value = client.recv().await;
        assert_eq!(value["type"], "output", "{value}");
        let text = value["text"].as_str().unwrap();
        if text.starts_with("SMALL") && text.ends_with('Q') {
            assert_eq!(value["replace"], true);
            let rows = text.split('\n').collect::<Vec<_>>();
            assert_eq!(rows.len(), 10);
            assert_eq!(rows[9], format!("{}Q", " ".repeat(29)));
            break;
        }
    }
    client
        .send(json!({"operation":"write","text":"exit\r"}))
        .await;
    loop {
        if client.recv().await["type"] == "exit" {
            break;
        }
    }
    client.success().await;
}
