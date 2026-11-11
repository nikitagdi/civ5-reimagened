//! The Node side: `node node/main.mjs`, one JSON object per line over stdin/stdout.
//!
//! ```text
//! request   {"id": 1, "cmd": "open", ...}
//! response  {"id": 1, "ok": true, "result": {...}}  |  {"id": 1, "ok": false, "error": {"code", "message"}}
//! ```
//!
//! One process holds one Chromium page; requests go strictly one after another. Node is found
//! as `node` on `PATH` or through `CIV5_NODE`; the scripts sit next to this crate (`node/`).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

use crate::errors::{ConvError, Place, codes};

/// The folder of the Node scripts.
pub fn node_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("node")
}

/// The Node executable.
pub fn node_exe() -> String {
    std::env::var("CIV5_NODE").unwrap_or_else(|_| "node".to_string())
}

/// Whether Node and Playwright's Chromium are usable here (for tests that skip without them).
pub fn available() -> Result<(), String> {
    let out = Command::new(node_exe())
        .arg("--version")
        .output()
        .map_err(|e| format!("node is not runnable: {e}"))?;
    if !out.status.success() {
        return Err("node --version failed".into());
    }
    let dir = node_dir();
    if !dir.join("node_modules").join("css-tree").exists() {
        return Err("tools/ui-convert/node has no node_modules (npm ci)".into());
    }
    if !dir
        .join("..")
        .join("..")
        .join("playtest")
        .join("node_modules")
        .join("playwright")
        .exists()
    {
        return Err("tools/playtest has no node_modules (npm ci)".into());
    }
    Ok(())
}

fn env_error(code: &str, why: impl Into<String>) -> ConvError {
    ConvError::new(code, Place::default(), why)
}

/// A running Node process.
pub struct NodeClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl NodeClient {
    /// Starts `node main.mjs`.
    pub fn start() -> Result<NodeClient, ConvError> {
        let main = node_dir().join("main.mjs");
        let mut child = Command::new(node_exe())
            .arg(&main)
            .current_dir(node_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| env_error(codes::NODE, format!("node did not start: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| env_error(codes::NODE, "no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| env_error(codes::NODE, "no stdout"))?;
        Ok(NodeClient {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    /// Sends one request and waits for its answer. `fields` is an object; `cmd` and `id` are added.
    pub fn call(&mut self, cmd: &str, fields: Value) -> Result<Value, ConvError> {
        let id = self.next_id;
        self.next_id += 1;
        let mut req = match fields {
            Value::Object(m) => m,
            Value::Null => serde_json::Map::new(),
            _ => return Err(env_error(codes::PROTOCOL, "a request is an object")),
        };
        req.insert("id".into(), json!(id));
        req.insert("cmd".into(), json!(cmd));
        let line = serde_json::to_string(&Value::Object(req))
            .map_err(|e| env_error(codes::PROTOCOL, e.to_string()))?;
        let io = |e: std::io::Error| env_error(codes::NODE, format!("node pipe: {e}"));
        self.stdin.write_all(line.as_bytes()).map_err(io)?;
        self.stdin.write_all(b"\n").map_err(io)?;
        self.stdin.flush().map_err(io)?;
        let mut answer = String::new();
        loop {
            answer.clear();
            let n = self.stdout.read_line(&mut answer).map_err(io)?;
            if n == 0 {
                return Err(env_error(
                    codes::NODE,
                    format!("node exited during `{cmd}`"),
                ));
            }
            if !answer.trim().is_empty() {
                break;
            }
        }
        let v: Value = serde_json::from_str(answer.trim())
            .map_err(|e| env_error(codes::PROTOCOL, format!("bad answer to `{cmd}`: {e}")))?;
        if v.get("id") != Some(&json!(id)) {
            return Err(env_error(
                codes::PROTOCOL,
                format!("answer to `{cmd}` has another id"),
            ));
        }
        if v.get("ok") == Some(&Value::Bool(true)) {
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
        let err = v.get("error").cloned().unwrap_or(Value::Null);
        let code = err
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or(codes::NODE)
            .to_string();
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Err(env_error(&code, format!("{cmd}: {msg}")))
    }

    /// Closes the session and waits for the process.
    pub fn close(mut self) {
        let _ = self.call("close", Value::Null);
        let _ = self.child.wait();
    }
}

impl Drop for NodeClient {
    fn drop(&mut self) {
        // A process that was not closed (an error path) is stopped; `close` already waited.
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
