//! Whole-transcript extraction (design: team-aware-learning-memory §6).
//!
//! `build_session_packet` only reads the final assistant message, so a
//! workflow repeated across sessions, or a skill that users keep correcting,
//! is invisible to it. This module reads the whole transcript JSONL and keeps
//! the parts skill discovery needs: user prompts, documentation artifacts,
//! Bash commands, the tool sequence, Skill invocations, and the corrections
//! users make after a skill ran.
//!
//! Parsing is bounded and lenient. At most [`MAX_TRANSCRIPT_BYTES`] are read,
//! malformed or torn lines are skipped, and every collection and string is
//! capped, so a hostile or huge transcript cannot grow the worker's memory.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

pub(crate) const MAX_TRANSCRIPT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PROMPTS: usize = 200;
const MAX_COMMANDS: usize = 200;
const MAX_ARTIFACTS: usize = 100;
const MAX_TOOL_STEPS: usize = 2_000;
const MAX_SKILL_USES: usize = 50;
const MAX_CORRECTIONS_PER_USE: usize = 10;
const PROMPT_CHARS: usize = 1_000;
const COMMAND_CHARS: usize = 200;
const PATH_CHARS: usize = 200;
const CORRECTION_CHARS: usize = 300;
/// Only the next few user prompts after a skill can be a correction of it.
const CORRECTION_WINDOW: usize = 6;
const ARTIFACT_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];
/// Built-in slash commands that are not skills.
const BUILTIN_COMMANDS: &[&str] = &[
    "clear",
    "compact",
    "config",
    "cost",
    "exit",
    "help",
    "init",
    "login",
    "logout",
    "mcp",
    "memory",
    "model",
    "permissions",
    "plugin",
    "resume",
    "status",
];
/// A user prompt that starts with one of these redirects the agent.
const CORRECTION_PREFIXES: &[&str] = &["no,", "no.", "no ", "nope", "stop", "wait,"];
/// A user prompt that contains one of these rejects or redirects prior work.
const CORRECTION_PHRASES: &[&str] = &[
    "don't",
    "do not",
    "dont ",
    "that's not",
    "thats not",
    "not what i",
    "wrong",
    "incorrect",
    "instead",
    "you missed",
    "you forgot",
    "should have",
    "shouldn't",
    "should not",
    "revert",
    "undo",
    "redo",
    "try again",
    "rather than",
];

/// One Skill invocation and the corrections that followed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillUse {
    pub skill: String,
    #[serde(default)]
    pub corrections: Vec<String>,
}

/// What a whole transcript says about how a session worked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranscriptDigest {
    /// Real user prompts in order; tool results and system tags excluded.
    pub prompts: Vec<String>,
    /// Paths written or edited under `docs/`, `reports/` or ending `.md`.
    pub artifacts: Vec<String>,
    pub commands: Vec<String>,
    /// Every tool use in order, e.g. `Read`, `Write`, `Bash:git`, `Skill:kbd-plan`.
    pub tool_sequence: Vec<String>,
    pub skill_uses: Vec<SkillUse>,
}

/// Read and digest a transcript. A missing or unreadable file digests to nothing.
pub(crate) fn parse_file(path: &Path) -> TranscriptDigest {
    match File::open(path) {
        Ok(file) => parse_reader(file),
        Err(_) => TranscriptDigest::default(),
    }
}

pub(crate) fn parse_reader(reader: impl Read) -> TranscriptDigest {
    let mut state = Parser::default();
    for line in BufReader::new(reader.take(MAX_TRANSCRIPT_BYTES)).split(b'\n') {
        let Ok(bytes) = line else { break };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        state.line(&value);
    }
    state.digest
}

#[derive(Default)]
struct Parser {
    digest: TranscriptDigest,
    /// Index into `skill_uses` and the user prompts seen since it ran.
    open_skill: Option<(usize, usize)>,
}

impl Parser {
    fn line(&mut self, value: &Value) {
        if value.get("isMeta").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let role = value
            .pointer("/message/role")
            .or_else(|| value.get("role"))
            .or_else(|| value.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(content) = value
            .pointer("/message/content")
            .or_else(|| value.get("content"))
        else {
            return;
        };
        match role {
            "user" => self.user(content),
            "assistant" => self.assistant(content),
            _ => {}
        }
    }

    fn user(&mut self, content: &Value) {
        if let Some(text) = content.as_str() {
            self.prompt(text);
            return;
        }
        let Some(blocks) = content.as_array() else {
            return;
        };
        let text = blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        // A message made only of tool results is the harness talking, not the user.
        if !text.is_empty() {
            self.prompt(&text);
        }
    }

    fn prompt(&mut self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(name) = slash_command(text) {
            if !BUILTIN_COMMANDS.contains(&name.as_str()) {
                self.skill(&name);
            }
            return;
        }
        if text.starts_with('<') {
            // `<system-reminder>`, `<local-command-stdout>` and similar harness tags.
            return;
        }
        if self.digest.prompts.len() < MAX_PROMPTS {
            self.digest.prompts.push(truncate(text, PROMPT_CHARS));
        }
        if let Some((index, seen)) = self.open_skill {
            if is_correction(text) {
                let corrections = &mut self.digest.skill_uses[index].corrections;
                if corrections.len() < MAX_CORRECTIONS_PER_USE {
                    corrections.push(truncate(text, CORRECTION_CHARS));
                }
            }
            self.open_skill = (seen + 1 < CORRECTION_WINDOW).then_some((index, seen + 1));
        }
    }

    fn assistant(&mut self, content: &Value) {
        let Some(blocks) = content.as_array() else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(name) = block.get("name").and_then(Value::as_str) else {
                continue;
            };
            let input = block.get("input").unwrap_or(&Value::Null);
            self.tool(name, input);
        }
    }

    fn tool(&mut self, name: &str, input: &Value) {
        let token = match name {
            "Skill" => {
                let skill = input
                    .get("skill")
                    .or_else(|| input.get("name"))
                    .and_then(Value::as_str)
                    .map(normalize_skill)
                    .filter(|skill| !skill.is_empty());
                match skill {
                    Some(skill) => {
                        self.skill(&skill);
                        format!("Skill:{skill}")
                    }
                    None => "Skill".to_owned(),
                }
            }
            "Bash" => {
                let command = input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                if !command.is_empty() && self.digest.commands.len() < MAX_COMMANDS {
                    self.digest.commands.push(truncate(command, COMMAND_CHARS));
                }
                match command.split_whitespace().next() {
                    Some(program) => format!("Bash:{}", truncate(program, 40)),
                    None => "Bash".to_owned(),
                }
            }
            _ => {
                if ARTIFACT_TOOLS.contains(&name) {
                    self.artifact(input);
                }
                name.to_owned()
            }
        };
        if self.digest.tool_sequence.len() < MAX_TOOL_STEPS {
            self.digest.tool_sequence.push(token);
        }
    }

    fn artifact(&mut self, input: &Value) {
        let Some(path) = input
            .get("file_path")
            .or_else(|| input.get("notebook_path"))
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
        else {
            return;
        };
        let path = truncate(path.trim(), PATH_CHARS);
        if is_documentation_path(&path)
            && self.digest.artifacts.len() < MAX_ARTIFACTS
            && !self.digest.artifacts.contains(&path)
        {
            self.digest.artifacts.push(path);
        }
    }

    fn skill(&mut self, skill: &str) {
        if self.digest.skill_uses.len() >= MAX_SKILL_USES {
            self.open_skill = None;
            return;
        }
        self.digest.skill_uses.push(SkillUse {
            skill: skill.to_owned(),
            corrections: Vec::new(),
        });
        self.open_skill = Some((self.digest.skill_uses.len() - 1, 0));
    }
}

/// `plugin:skill` and `/skill` both name the skill `skill`.
fn normalize_skill(raw: &str) -> String {
    let name = raw.trim().trim_start_matches('/');
    name.rsplit(':').next().unwrap_or(name).trim().to_owned()
}

/// A user prompt that is the harness echo of a slash command:
/// `<command-name>/kbd-plan</command-name>`.
fn slash_command(text: &str) -> Option<String> {
    let rest = text.split("<command-name>").nth(1)?;
    let name = rest.split("</command-name>").next()?;
    let name = normalize_skill(name);
    (!name.is_empty()).then_some(name)
}

fn is_documentation_path(path: &str) -> bool {
    let padded = format!("/{path}");
    padded.contains("/docs/") || padded.contains("/reports/") || path.ends_with(".md")
}

fn is_correction(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    CORRECTION_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        || CORRECTION_PHRASES
            .iter()
            .any(|phrase| lower.contains(phrase))
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
