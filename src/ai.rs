//! Commit messages and commit explanations written by a coding assistant installed on the
//! machine (Claude Code's `claude` or OpenAI's `codex`), run non-interactively with no tools:
//! gitcraft hands it the diff and the user's rules, and takes its answer. Each tool uses its own
//! login; gitcraft stores no key.

use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Assistant {
    #[default]
    Off,
    Claude,
    Codex,
}

impl Assistant {
    pub const ALL: [Assistant; 3] = [Assistant::Off, Assistant::Claude, Assistant::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Assistant::Off => "Off",
            Assistant::Claude => "Claude Code",
            Assistant::Codex => "Codex",
        }
    }

    pub fn program(self) -> Option<&'static str> {
        match self {
            Assistant::Off => None,
            Assistant::Claude => Some("claude"),
            Assistant::Codex => Some("codex"),
        }
    }

    /// Whether its program is on the PATH (the login shell's, as for git).
    pub fn installed(self) -> bool {
        self.program().is_some_and(on_path)
    }
}

pub const DEFAULT_COMMIT_RULES: &str = "Use Conventional Commits (feat:, fix:, refactor:, docs:, test:, chore:…), with a scope when it is obvious.
Summary line in the imperative mood, at most 72 characters, no period at the end.
If the change is not trivial, add a blank line and a short body saying what changed and why.
Write in the same language as the repository's recent commits.";

pub const DEFAULT_EXPLAIN_RULES: &str =
    "Explain what the commit changes and why it matters, for someone reviewing the history.
A one-sentence summary first, then a few short bullets. Mention risks or anything surprising.
Answer in the same language as the commit message.";

/// The most of a diff sent: assistants have limits, and a huge diff says little more.
const MAX_DIFF: usize = 60_000;

fn on_path(exe: &str) -> bool {
    let paths = crate::shell_path::path()
        .cloned()
        .or_else(|| std::env::var_os("PATH"));
    let Some(paths) = paths else { return false };
    std::env::split_paths(&paths).any(|dir| {
        let p = dir.join(exe);
        p.is_file() || (cfg!(windows) && dir.join(format!("{exe}.exe")).is_file())
    })
}

/// Cuts `text` to `max` bytes at a line end, saying so.
fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text[..end].rfind('\n').unwrap_or(end);
    format!(
        "{}\n[… diff cut here: {} more bytes not shown]",
        &text[..cut],
        text.len() - cut
    )
}

/// The prompt for a commit message of `diff` (with its `--stat`), in the style of `recent`.
pub fn commit_prompt(rules: &str, branch: &str, recent: &str, stat: &str, diff: &str) -> String {
    format!(
        "Write the git commit message for the changes below.\n\n\
         Rules:\n{rules}\n\n\
         Reply with the commit message only: no explanation before or after, no code fences, no quotes.\n\n\
         Branch: {branch}\n\n\
         Recent commit messages in this repository (for style and language):\n{recent}\n\n\
         Files changed:\n{stat}\n\n\
         Diff:\n{}",
        truncate(diff, MAX_DIFF)
    )
}

/// The prompt explaining a commit from its `git show`.
pub fn explain_prompt(rules: &str, show: &str) -> String {
    format!(
        "Explain this git commit.\n\nRules:\n{rules}\n\n\
         Plain text (short bullets with '-' are fine), no code fences, no headings.\n\n{}",
        truncate(show, MAX_DIFF)
    )
}

/// Runs `assistant` on `prompt` in `repo` and returns its answer.
pub fn run(assistant: Assistant, model: &str, repo: &Path, prompt: &str) -> Result<String, String> {
    let Some(program) = assistant.program() else {
        return Err("No assistant is chosen (Settings → AI)".to_owned());
    };
    let model = model.trim();
    let mut cmd = Command::new(program);
    cmd.current_dir(repo);
    // gitcraft's own output file for codex: its stdout carries progress, not just the answer.
    let out_file = std::env::temp_dir().join(format!(
        "gitcraft-ai-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    match assistant {
        Assistant::Claude => {
            cmd.args([
                "-p",
                "--output-format",
                "text",
                "--no-session-persistence",
                "--tools",
                "",
            ]);
            if !model.is_empty() {
                cmd.args(["--model", model]);
            }
            // Started from a Claude Code terminal, it would think it is nested.
            cmd.env_remove("CLAUDECODE");
        }
        Assistant::Codex => {
            cmd.args([
                "exec",
                "--ephemeral",
                "--sandbox",
                "read-only",
                "--skip-git-repo-check",
                "--color",
                "never",
                "-o",
            ]);
            cmd.arg(&out_file);
            if !model.is_empty() {
                cmd.args(["-m", model]);
            }
            cmd.arg("-");
        }
        Assistant::Off => unreachable!(),
    }
    crate::shell_path::apply(&mut cmd);
    crate::workspace::apply_tool(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start {program}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        // A closed pipe shows up as the program's own error below.
        let _ = stdin.write_all(prompt.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{program} failed: {e}"))?;
    let answer = match assistant {
        Assistant::Codex => {
            let text = std::fs::read_to_string(&out_file).unwrap_or_default();
            let _ = std::fs::remove_file(&out_file);
            text
        }
        _ => String::from_utf8_lossy(&out.stdout).into_owned(),
    };
    let answer = clean(&answer);
    if out.status.success() && !answer.is_empty() {
        Ok(answer)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let last: Vec<&str> = err
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let tail = last[last.len().saturating_sub(3)..].join("\n");
        Err(if tail.is_empty() {
            format!("{program} gave no answer ({})", out.status)
        } else {
            format!("{program}: {tail}")
        })
    }
}

/// The answer without code fences or quotes around it.
fn clean(text: &str) -> String {
    let mut t = text.trim();
    if let Some(rest) = t.strip_prefix("```") {
        // Drop the fence's language tag line.
        t = rest.split_once('\n').map_or("", |(_, r)| r);
        t = t.trim_end().strip_suffix("```").unwrap_or(t);
    }
    let t = t.trim();
    let t = t
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t);
    t.trim().to_owned()
}

/// A commit message split into its summary and body.
pub fn split_message(msg: &str) -> (String, String) {
    let msg = msg.trim();
    match msg.split_once('\n') {
        Some((summary, body)) => (summary.trim().to_owned(), body.trim().to_owned()),
        None => (msg.to_owned(), String::new()),
    }
}

/// Everything the commit-message prompt needs from the repository: the staged changes, or every
/// change when nothing is staged (as "Stage all and commit" would commit them).
pub fn commit_context(repo: &Path) -> Result<(String, String, String, String), String> {
    let git = |args: &[&str]| crate::git::run(Some(repo), args);
    let branch = git(&["branch", "--show-current"])
        .unwrap_or_default()
        .trim()
        .to_owned();
    let recent = git(&["log", "-12", "--format=%B%x1e"])
        .unwrap_or_default()
        .split('\x1e')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| format!("---\n{m}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut stat = git(&["diff", "--cached", "--stat"])?;
    let mut diff = git(&["diff", "--cached"])?;
    if diff.trim().is_empty() {
        stat = git(&["diff", "HEAD", "--stat"]).or_else(|_| git(&["diff", "--stat"]))?;
        diff = git(&["diff", "HEAD"]).or_else(|_| git(&["diff"]))?;
        let untracked = git(&["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
        if !untracked.trim().is_empty() {
            stat.push_str("\nNew files:\n");
            stat.push_str(&untracked);
            for path in untracked.lines().take(20) {
                if let Ok(content) = std::fs::read_to_string(repo.join(path)) {
                    diff.push_str(&format!(
                        "\n--- new file {path}\n{}",
                        truncate(&content, 4000)
                    ));
                }
            }
        }
    }
    if diff.trim().is_empty() && stat.trim().is_empty() {
        return Err("There are no changes to describe".to_owned());
    }
    Ok((branch, recent, stat, diff))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_answers() {
        assert_eq!(clean("```text\nfeat: x\n\nbody\n```\n"), "feat: x\n\nbody");
        assert_eq!(clean("  \"fix: y\"  "), "fix: y");
        assert_eq!(clean("chore: z"), "chore: z");
    }

    #[test]
    fn splits_messages() {
        assert_eq!(
            split_message("feat: a\n\nline 1\nline 2\n"),
            ("feat: a".to_owned(), "line 1\nline 2".to_owned())
        );
        assert_eq!(
            split_message("fix: b"),
            ("fix: b".to_owned(), String::new())
        );
    }

    #[test]
    fn truncates_at_a_line() {
        let text = "aaaa\nbbbb\ncccc\n";
        let cut = truncate(text, 7);
        assert!(cut.starts_with("aaaa\n[… diff cut here"));
        assert_eq!(truncate(text, 100), text);
    }

    #[test]
    fn prompt_has_rules_and_diff() {
        let p = commit_prompt("RULE", "main", "---\nfeat: old", "a | 1 +", "+x");
        assert!(p.contains("RULE") && p.contains("+x") && p.contains("feat: old"));
    }
}
