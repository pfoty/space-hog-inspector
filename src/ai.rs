//! Asks the AI agent already installed on this Mac (Claude Code or Codex)
//! whether an item is safe to remove.
//!
//! The agent runs headless under the user's own sign-in. It is sent only
//! facts gathered here (path, size, type, age and the largest items inside),
//! never file contents. Claude Code runs with every tool switched off; Codex
//! has no such switch, so it runs in its read-only sandbox instead.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::platform;
use crate::scanner::{Node, format_size};

/// Longest an agent may take before it is stopped.
const TIMEOUT: Duration = Duration::from_secs(90);
/// How many of a folder's largest items are described.
pub const TOP_ITEMS: usize = 15;
/// Claude model used for answers: fast, and good at this.
const CLAUDE_MODEL: &str = "sonnet";

const SYSTEM_PROMPT: &str = "You assess whether a file or folder on a Mac can be removed to free disk space. \
You are given only its path, size, type, age and largest contents; you cannot inspect it. \
Do not run commands or read files: answer from the facts given. \
Say what it most likely is, whether removing it is safe, what could break or be lost, whether macOS or an app \
recreates it on its own, and the safest way to remove it (for example an app's own setting or command), or an \
empty string if moving it to the Trash is fine. Be concise: what_it_is at most two sentences, at most four risks \
of one sentence each, safer_way at most two sentences. If the facts aren't enough to tell, say so and choose caution.";

/// The answer's shape. Codex needs every property required and no extras.
const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"properties":{"verdict":{"type":"string","enum":["safe","likely_safe","caution","keep"]},"what_it_is":{"type":"string"},"risks":{"type":"array","items":{"type":"string"}},"regenerates":{"type":"boolean"},"safer_way":{"type":"string"}},"required":["verdict","what_it_is","risks","regenerates","safer_way"]}"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Claude, Provider::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude Code",
            Provider::Codex => "Codex",
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    /// What to run in Terminal to sign in.
    fn sign_in(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex login",
        }
    }
}

/// An installed agent.
#[derive(Clone, Debug)]
pub struct Agent {
    pub provider: Provider,
    pub exe: PathBuf,
}

/// Finds the installed agents: the usual locations first, then the system's
/// own lookup for installs elsewhere (for example under nvm).
pub fn detect() -> Vec<Agent> {
    Provider::ALL
        .into_iter()
        .filter_map(|provider| {
            let exe = platform::agent_candidates(provider.binary())
                .into_iter()
                .find(|p| platform::is_executable(p))
                .or_else(|| platform::find_with_shell(provider.binary()))?;
            Some(Agent { provider, exe })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Safe,
    LikelySafe,
    Caution,
    Keep,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Safe => "SAFE TO REMOVE",
            Verdict::LikelySafe => "LIKELY SAFE",
            Verdict::Caution => "CAUTION",
            Verdict::Keep => "KEEP",
        }
    }
}

/// The agent's answer.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Assessment {
    pub verdict: Verdict,
    pub what_it_is: String,
    pub risks: Vec<String>,
    /// Whether macOS or an app recreates it on its own.
    pub regenerates: bool,
    /// A better way to remove it than the Trash, or empty.
    pub safer_way: String,
}

/// Everything the agent is told about an item.
#[derive(Clone, Debug)]
pub struct Facts {
    /// Full path, with the home folder shown as `~`.
    pub path: String,
    pub is_dir: bool,
    pub kind: &'static str,
    pub size: u64,
    pub files: u64,
    pub modified_days_ago: Option<u64>,
    /// (name, size, is folder) of the largest items inside, largest first.
    pub items: Vec<(String, u64, bool)>,
}

impl Facts {
    pub fn gather(node: &Node, fs_path: &Path, home: Option<&Path>) -> Facts {
        let modified_days_ago = fs::symlink_metadata(fs_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .map(|d| d.as_secs() / 86_400);
        Facts {
            path: display_path(fs_path, home),
            is_dir: node.is_dir,
            kind: node.kind.label(),
            size: node.size,
            files: node.files,
            modified_days_ago,
            items: node.children.iter().take(TOP_ITEMS).map(|c| (c.name.to_string(), c.size, c.is_dir)).collect(),
        }
    }

    fn prompt(&self) -> String {
        let mut s = format!(
            "{}: {}\nType: {}\nSize on disk: {}\n",
            if self.is_dir { "Folder" } else { "File" },
            self.path,
            self.kind,
            format_size(self.size),
        );
        if self.is_dir {
            s += &format!("Files inside: {}\n", self.files);
        }
        if let Some(days) = self.modified_days_ago {
            s += &format!("Last modified: {days} days ago\n");
        }
        if !self.items.is_empty() {
            s += "Largest items inside:\n";
            for (name, size, is_dir) in &self.items {
                s += &format!("- {name}{} ({})\n", if *is_dir { "/" } else { "" }, format_size(*size));
            }
        }
        s += "\nIs it safe to remove this to free up space?";
        s
    }
}

/// `path` with the home folder shown as `~`, so the user's account name
/// isn't sent.
fn display_path(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    path.display().to_string()
}

/// Asks `agent` about the item. Blocks until it answers, fails, takes too
/// long, or `cancel` is set, so run it off the UI thread.
pub fn ask(agent: &Agent, facts: &Facts, cancel: &AtomicBool) -> Result<Assessment, String> {
    let work = work_dir()?;
    match agent.provider {
        Provider::Claude => {
            let mut cmd = Command::new(&agent.exe);
            cmd.args(["-p", "--output-format", "json", "--no-session-persistence"])
                .args(["--tools", "", "--strict-mcp-config", "--setting-sources", ""])
                .args(["--model", CLAUDE_MODEL, "--system-prompt", SYSTEM_PROMPT, "--json-schema", SCHEMA]);
            let (ok, stdout, stderr) = run(cmd, agent, &work, &facts.prompt(), cancel)?;
            parse_claude(&stdout).map_err(|e| explain(agent.provider, &e, &stderr, ok))
        }
        Provider::Codex => {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let id = format!("{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
            let schema = work.join(format!("schema-{id}.json"));
            let answer = work.join(format!("answer-{id}.json"));
            fs::write(&schema, SCHEMA).map_err(|e| format!("Couldn't write {}: {e}", schema.display()))?;
            let mut cmd = Command::new(&agent.exe);
            cmd.args(["exec", "--ephemeral", "--skip-git-repo-check", "--ignore-user-config"])
                .args(["--sandbox", "read-only", "--color", "never", "-C"])
                .arg(&work)
                .arg("--output-schema")
                .arg(&schema)
                .arg("-o")
                .arg(&answer)
                .arg("-");
            let prompt = format!("{SYSTEM_PROMPT}\n\n{}", facts.prompt());
            let result = run(cmd, agent, &work, &prompt, cancel);
            let text = fs::read_to_string(&answer).unwrap_or_default();
            let _ = fs::remove_file(&schema);
            let _ = fs::remove_file(&answer);
            let (ok, _, stderr) = result?;
            parse_codex(&text).map_err(|e| explain(agent.provider, &e, &stderr, ok))
        }
    }
}

/// An empty folder to run agents in, so they pick up no project files.
fn work_dir() -> Result<PathBuf, String> {
    let dir = platform::app_cache_dir().ok_or("HOME is not set")?.join("ai");
    fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Runs `cmd` with `input` on stdin. Returns whether it exited cleanly, and
/// its stdout and stderr.
fn run(mut cmd: Command, agent: &Agent, dir: &Path, input: &str, cancel: &AtomicBool) -> Result<(bool, String, String), String> {
    let name = agent.provider.label();
    platform::hide_console(&mut cmd);
    let mut child = cmd
        .current_dir(dir)
        .env("PATH", platform::agent_path(&agent.exe))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start {name} at {}: {e}", agent.exe.display()))?;
    // Read both pipes on their own threads so a full pipe can't stall the agent.
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let stdout = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out_pipe.read_to_string(&mut s);
        s
    });
    let stderr = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err_pipe.read_to_string(&mut s);
        s
    });
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.as_bytes());
    }

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) || started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(if cancel.load(Ordering::Relaxed) {
                "Cancelled.".into()
            } else {
                format!("{name} didn't answer within {} seconds.", TIMEOUT.as_secs())
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    Ok((status.success(), stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default()))
}

/// Reads Claude Code's `--output-format json` result.
fn parse_claude(stdout: &str) -> Result<Assessment, String> {
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|_| first_line(stdout).unwrap_or_default())?;
    let failed = v["is_error"].as_bool().unwrap_or(false) || v["subtype"].as_str().is_some_and(|s| s != "success");
    let result = v["result"].as_str().unwrap_or_default();
    if failed {
        return Err(result.to_string());
    }
    if !v["structured_output"].is_null() {
        return serde_json::from_value(v["structured_output"].clone()).map_err(|e| format!("Unexpected answer: {e}"));
    }
    serde_json::from_str(result).map_err(|e| format!("Unexpected answer: {e}"))
}

/// Reads the final message Codex wrote with `-o`.
fn parse_codex(text: &str) -> Result<Assessment, String> {
    if text.trim().is_empty() {
        return Err(String::new());
    }
    serde_json::from_str(text.trim()).map_err(|e| format!("Unexpected answer: {e}"))
}

/// Turns a failure into a message the user can act on.
fn explain(provider: Provider, error: &str, stderr: &str, exited_ok: bool) -> String {
    let all = format!("{error}\n{stderr}").to_lowercase();
    let name = provider.label();
    if ["not logged in", "log in", "login", "sign in", "authenticat", "unauthorized", "401"].iter().any(|k| all.contains(k)) {
        return format!("{name} isn't signed in. Open Terminal and run `{}` once to sign in.", provider.sign_in());
    }
    if !error.trim().is_empty() {
        return format!("{name}: {}", truncate(error.trim(), 300));
    }
    match last_line(stderr) {
        Some(line) => format!("{name} failed: {}", truncate(&line, 300)),
        None if exited_ok => format!("{name} returned no answer."),
        None => format!("{name} stopped without an answer."),
    }
}

fn first_line(s: &str) -> Option<String> {
    s.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

fn last_line(s: &str) -> Option<String> {
    s.lines().map(str::trim).rfind(|l| !l.is_empty()).map(str::to_string)
}

fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Assessment {
        Assessment {
            verdict: Verdict::LikelySafe,
            what_it_is: "Spotify's cache.".into(),
            risks: vec!["Offline songs may need downloading again.".into()],
            regenerates: true,
            safer_way: "Use Settings > Storage > Clear cache in Spotify.".into(),
        }
    }

    const ANSWER: &str = r#"{"verdict":"likely_safe","what_it_is":"Spotify's cache.","risks":["Offline songs may need downloading again."],"regenerates":true,"safer_way":"Use Settings > Storage > Clear cache in Spotify."}"#;

    #[test]
    fn reads_claude_structured_output() {
        let out = format!(r#"{{"type":"result","subtype":"success","is_error":false,"result":"ignored","structured_output":{ANSWER}}}"#);
        assert_eq!(parse_claude(&out).unwrap(), sample());
    }

    #[test]
    fn falls_back_to_claude_result_text() {
        let out = serde_json::json!({"subtype": "success", "is_error": false, "result": ANSWER}).to_string();
        assert_eq!(parse_claude(&out).unwrap(), sample());
    }

    #[test]
    fn reports_claude_errors() {
        let out = r#"{"subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
        let err = parse_claude(out).unwrap_err();
        assert!(explain(Provider::Claude, &err, "", false).contains("isn't signed in"));
    }

    #[test]
    fn reads_codex_answer() {
        assert_eq!(parse_codex(&format!("{ANSWER}\n")).unwrap(), sample());
        assert!(parse_codex("").is_err());
    }

    #[test]
    fn explains_failures_without_output() {
        assert_eq!(explain(Provider::Codex, "", "warn\nboom\n", false), "Codex failed: boom");
        assert_eq!(explain(Provider::Codex, "", "", false), "Codex stopped without an answer.");
    }

    #[test]
    fn hides_the_home_folder() {
        let home = Path::new("/Users/someone");
        assert_eq!(display_path(Path::new("/Users/someone/Library/Caches"), Some(home)), "~/Library/Caches");
        assert_eq!(display_path(home, Some(home)), "~");
        assert_eq!(display_path(Path::new("/Applications"), Some(home)), "/Applications");
    }

    #[test]
    fn prompt_lists_largest_items_without_the_home_folder() {
        let node = Node::dir(
            "com.spotify.client",
            vec![Node::file("a.bin", 3_000), Node::dir("Data", vec![Node::file("b", 9_000)])],
        );
        let facts = Facts::gather(&node, Path::new("/Users/someone/Library/Caches/com.spotify.client"), Some(Path::new("/Users/someone")));
        let prompt = facts.prompt();
        assert!(prompt.starts_with("Folder: ~/Library/Caches/com.spotify.client\n"));
        assert!(prompt.contains("- Data/ (9.0 KB)\n- a.bin (3.0 KB)\n"));
        assert!(!prompt.contains("/Users/someone"));
    }

    #[test]
    fn caps_the_number_of_items() {
        let children = (0..TOP_ITEMS + 5).map(|i| Node::file(format!("f{i}"), 100 + i as u64)).collect();
        let facts = Facts::gather(&Node::dir("big", children), Path::new("/tmp/big"), None);
        assert_eq!(facts.items.len(), TOP_ITEMS);
    }

    #[test]
    fn schema_is_valid_json() {
        let v: serde_json::Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(v["required"].as_array().unwrap().len(), 5);
    }
}
