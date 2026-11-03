use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct McpClient {
    child: Child,
    responses: Receiver<String>,
    next_id: u64,
}

impl McpClient {
    fn start(binary: &str, prefix: &[&str]) -> Self {
        let mut child = Command::new(binary)
            .args(prefix)
            .args(["serve", "--mcp", "--transport", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start MCP server");
        let stdout = child.stdout.take().unwrap();
        let (sender, responses) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            responses,
            next_id: 0,
        };
        client.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "dispatch-regression", "version": "1"}
            }),
        );
        client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        client
    }

    fn send(&mut self, message: Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &message).unwrap();
        writeln!(stdin).unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params
        }));
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let line = self
                .responses
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("MCP response before timeout");
            let response: Value = serde_json::from_str(&line).expect("MCP JSON response");
            if response["id"] == self.next_id {
                assert!(response.get("error").is_none(), "{response}");
                return response["result"].clone();
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.child.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Exercises the subprocess used by execute=true without credentials or network I/O.
pub fn assert_adrive_mcp_dispatch(binary: &str, prefix: &[&str]) {
    let mut client = McpClient::start(binary, prefix);
    let plan = client.request(
        "tools/call",
        json!({"name": "ve_adrive_ls", "arguments": {"describe": true}}),
    );
    assert_eq!(
        plan["structuredContent"]["execution_status"],
        "planned_not_executed"
    );

    for (name, arguments, command) in [
        (
            "ve_adrive_capabilities",
            json!({"execute": true}),
            "ve-adrive capabilities",
        ),
        (
            "ve_adrive_ls",
            // A profile value equal to the public command must be preserved.
            json!({"execute": true, "describe": true, "profile": "ve-adrive"}),
            "ve-adrive ls",
        ),
    ] {
        let executed = client.request("tools/call", json!({"name": name, "arguments": arguments}));
        assert_eq!(executed["isError"], false, "{executed}");
        let output = &executed["structuredContent"];
        assert_eq!(output["exit_code"], 0, "{output}");
        let envelope: Value = serde_json::from_str(output["stdout"].as_str().unwrap()).unwrap();
        assert_eq!(envelope["success"], true, "{envelope}");
        assert_eq!(envelope["command"], command);
    }
}
