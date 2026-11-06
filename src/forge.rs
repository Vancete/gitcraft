//! Pull requests through the forges' own command-line tools: `gh` for GitHub, `glab` for
//! GitLab. They hold the user's login, so gitcraft never stores a token.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::git::Remote;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forge {
    GitHub,
    GitLab,
}

impl Forge {
    pub fn tool(self) -> &'static str {
        match self {
            Forge::GitHub => "gh",
            Forge::GitLab => "glab",
        }
    }

    /// "pull request" or "merge request".
    pub fn noun(self) -> &'static str {
        match self {
            Forge::GitHub => "pull request",
            Forge::GitLab => "merge request",
        }
    }
}

/// The forge hosting `origin` (or the first remote), from its URL.
pub fn detect(remotes: &[Remote]) -> Option<Forge> {
    let remote = remotes
        .iter()
        .find(|r| r.name == "origin")
        .or_else(|| remotes.first())?;
    // GitHub by the host alone: a GitLab project may well be called `me.github.io`. GitLab can
    // also live under a path of another host (`https://example.org/gitlab/…`).
    // A host naming GitLab wins (`github-mirror.gitlab.example.org`).
    // A local path has no host, and `C:/…` only a drive's: neither is on any forge, whatever
    // folder names it goes through.
    let host = location(&remote.url)
        .map(|(host, _)| host)
        .filter(|host| host.len() > 1)?;
    if host.contains("gitlab") {
        Some(Forge::GitLab)
    } else if host.contains("github") {
        Some(Forge::GitHub)
    } else if remote.url.to_lowercase().contains("gitlab") {
        Some(Forge::GitLab)
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    /// The branch it proposes (head / source branch).
    pub branch: String,
    /// Its latest commit.
    pub sha: String,
    pub author: String,
    pub url: String,
    pub draft: bool,
}

pub(crate) fn tool(forge: Forge, repo: &Path, args: &[&str]) -> Result<String, String> {
    tool_with_input(forge, repo, args, None)
}

/// [`tool`], with `input` on its stdin.
pub(crate) fn tool_with_input(
    forge: Forge,
    repo: &Path,
    args: &[&str],
    input: Option<&str>,
) -> Result<String, String> {
    let mut cmd = tool_command(forge);
    cmd.current_dir(repo).args(args);
    let not_installed = |_| {
        format!(
            "{} isn't installed: gitcraft uses it for {}s",
            forge.tool(),
            forge.noun()
        )
    };
    let out = match input {
        None => cmd.output().map_err(not_installed)?,
        Some(input) => {
            let mut child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(not_installed)?;
            if let Some(mut stdin) = child.stdin.take() {
                // A tool that stops reading has stopped for a reason, which its exit code and
                // error output below tell better than the broken pipe would.
                let _ = stdin.write_all(input.as_bytes());
            }
            child.wait_with_output().map_err(|e| e.to_string())?
        }
    };
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(failure(forge, &out))
    }
}

/// The forge's tool, set to answer this repository in plain JSON.
fn tool_command(forge: Forge) -> Command {
    let mut cmd = Command::new(forge.tool());
    cmd.env("GH_PROMPT_DISABLED", "1").env("NO_PROMPT", "1");
    // What it prints is read as JSON: `CLICOLOR_FORCE` or `GH_FORCE_TTY` in the user's
    // environment would colour it even into a pipe, escape codes no JSON reader takes.
    cmd.env_remove("CLICOLOR_FORCE")
        .env_remove("GH_FORCE_TTY")
        .env("NO_COLOR", "1");
    // About the repository it runs in and the host it is given, not one the environment names:
    // `GH_REPO` would answer for another repository, `GH_HOST` send `gh api` (and `-R owner/repo`)
    // to another host than github.com.
    cmd.env_remove("GH_REPO").env_remove("GH_HOST");
    crate::shell_path::apply(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// What a tool that failed said, or that it failed: an empty error would read as nothing wrong.
fn failure(forge: Forge, out: &std::process::Output) -> String {
    let said = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_owned();
    let stderr = said(&out.stderr);
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = said(&out.stdout);
    if !stdout.is_empty() {
        return stdout;
    }
    format!("{} failed ({})", forge.tool(), out.status)
}

pub fn list(repo: &Path, forge: Forge) -> Result<Vec<PullRequest>, String> {
    let out = match forge {
        Forge::GitHub => tool(
            forge,
            repo,
            &[
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                "100",
                "--json",
                "number,title,headRefName,headRefOid,author,url,isDraft",
            ],
        )?,
        // As many as of GitHub's: glab's own page holds only 30.
        Forge::GitLab => tool(
            forge,
            repo,
            &["mr", "list", "--per-page", "100", "--output", "json"],
        )?,
    };
    parse(forge, &out)
}

fn parse(forge: Forge, json: &str) -> Result<Vec<PullRequest>, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("unexpected {} output: {e}", forge.tool()))?;
    let items = value.as_array().cloned().unwrap_or_default();
    let s = |v: &serde_json::Value, path: &[&str]| -> String {
        let mut cur = v;
        for key in path {
            match cur.get(*key) {
                Some(next) => cur = next,
                None => return String::new(),
            }
        }
        cur.as_str().map(str::to_owned).unwrap_or_default()
    };
    Ok(items
        .iter()
        .map(|item| match forge {
            Forge::GitHub => PullRequest {
                number: item.get("number").and_then(|n| n.as_u64()).unwrap_or(0),
                title: s(item, &["title"]),
                branch: s(item, &["headRefName"]),
                sha: s(item, &["headRefOid"]),
                author: {
                    let author = s(item, &["author", "login"]);
                    match author.strip_prefix("app/") {
                        Some(app) => app.to_owned(),
                        None => author,
                    }
                },
                url: s(item, &["url"]),
                draft: item
                    .get("isDraft")
                    .and_then(|d| d.as_bool())
                    .unwrap_or(false),
            },
            Forge::GitLab => PullRequest {
                number: item.get("iid").and_then(|n| n.as_u64()).unwrap_or(0),
                title: s(item, &["title"]),
                branch: s(item, &["source_branch"]),
                sha: s(item, &["sha"]),
                author: s(item, &["author", "username"]),
                url: s(item, &["web_url"]),
                draft: item.get("draft").and_then(|d| d.as_bool()).unwrap_or(false),
            },
        })
        .collect())
}

/// Opens a pull request from `branch` into `base`; returns what the tool printed (its URL).
pub fn create(
    repo: &Path,
    forge: Forge,
    branch: &str,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<String, String> {
    let mut args: Vec<&str> = match forge {
        Forge::GitHub => vec![
            "pr", "create", "--head", branch, "--base", base, "--title", title, "--body", body,
        ],
        Forge::GitLab => vec![
            "mr",
            "create",
            "--source-branch",
            branch,
            "--target-branch",
            base,
            "--title",
            title,
            "--description",
            body,
            "--yes",
        ],
    };
    if draft {
        args.push("--draft");
    }
    tool(forge, repo, &args).map(|out| out.trim().to_owned())
}

/// The web host and `owner/repo` path of a remote URL: `https://host/owner/repo.git`,
/// `git@host:owner/repo.git` or `ssh://git@host[:port]/owner/repo`.
pub fn location(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = match url.split_once("://") {
        Some((_, rest)) => {
            // The user part is only ever before the first `/`: an `@` in the path names no host.
            let (authority, path) = rest.split_once('/')?;
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            (host.split(':').next().unwrap_or(host), path)
        }
        None => {
            let rest = url.rsplit_once('@').map_or(url, |(_, r)| r);
            rest.split_once(':')?
        }
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || !path.contains('/') {
        return None;
    }
    // SSH over the HTTPS port goes to another name than the forge's own.
    let host = match host.to_lowercase().as_str() {
        "ssh.github.com" => "github.com".to_owned(),
        "altssh.gitlab.com" => "gitlab.com".to_owned(),
        host => host.to_owned(),
    };
    Some((host, path.to_owned()))
}

/// The port of an `http(s)://` URL, which its web pages are on too; an SSH port says nothing
/// about them.
fn http_port(url: &str) -> Option<&str> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split('/').next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (_, port) = host.split_once(':')?;
    (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())).then_some(port)
}

/// The web page of the repository at remote `url`: `https://github.com/owner/repo` for
/// `git@github.com:owner/repo.git`. `None` for a local path, which has no page.
pub fn web_url(url: &str) -> Option<String> {
    let (host, path) = location(url)?;
    // `C:/repos/x` reads as host `c`; a host that serves pages has a domain.
    if !host.contains('.') {
        return None;
    }
    let scheme = if url.trim_start().to_lowercase().starts_with("http://") {
        "http"
    } else {
        "https"
    };
    match http_port(url) {
        Some(port) => Some(format!("{scheme}://{host}:{port}/{path}")),
        None => Some(format!("{scheme}://{host}/{path}")),
    }
}

/// GitHub accounts' avatars by commit email, from the repository's latest commits (the API ties
/// each commit to the account its email belongs to). Through `gh` when it's installed (private
/// repositories, higher limits), else GitHub's public API.
pub fn github_avatars(repo: &Path, host: &str, path: &str) -> HashMap<String, String> {
    let endpoint = format!("repos/{path}/commits?per_page=100");
    let mut args = vec!["api", endpoint.as_str()];
    if host != "github.com" {
        args.extend(["--hostname", host]);
    }
    let json = tool(Forge::GitHub, repo, &args).or_else(|e| {
        if host != "github.com" {
            return Err(e);
        }
        ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("gitcraft")
            .build()
            .get(&format!("https://api.github.com/{endpoint}"))
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())
    });
    json.map(|j| parse_github_avatars(&j)).unwrap_or_default()
}

fn parse_github_avatars(json: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str(json) else {
        return out;
    };
    for item in &items {
        for (who, account) in [("author", "author"), ("committer", "committer")] {
            let email = item
                .pointer(&format!("/commit/{who}/email"))
                .and_then(|e| e.as_str());
            let avatar = item
                .pointer(&format!("/{account}/avatar_url"))
                .and_then(|a| a.as_str());
            if let (Some(email), Some(avatar)) = (email, avatar) {
                out.entry(email.trim().to_lowercase())
                    .or_insert_with(|| avatar.to_owned());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's environment can't colour the JSON read from the tools, nor point them at
    /// another repository or host.
    #[test]
    fn tools_answer_in_plain_json_about_this_repository() {
        for forge in [Forge::GitHub, Forge::GitLab] {
            let cmd = tool_command(forge);
            let envs: HashMap<String, Option<String>> = cmd
                .get_envs()
                .map(|(k, v)| {
                    let text = |s: &std::ffi::OsStr| s.to_string_lossy().into_owned();
                    (text(k), v.map(text))
                })
                .collect();
            for removed in ["CLICOLOR_FORCE", "GH_FORCE_TTY", "GH_REPO", "GH_HOST"] {
                assert_eq!(envs.get(removed), Some(&None), "{removed}");
            }
            assert_eq!(envs.get("NO_COLOR"), Some(&Some("1".to_owned())));
        }
    }

    #[test]
    fn remote_locations() {
        let gh = Some(("github.com".to_owned(), "owner/repo".to_owned()));
        assert_eq!(location("https://github.com/owner/repo.git"), gh);
        assert_eq!(location("https://user@github.com/owner/repo/"), gh);
        assert_eq!(location("git@github.com:owner/repo.git"), gh);
        assert_eq!(location("ssh://git@github.com:22/owner/repo"), gh);
        assert_eq!(
            location("https://gitlab.example.org/group/sub/proj.git"),
            Some(("gitlab.example.org".to_owned(), "group/sub/proj".to_owned()))
        );
        assert_eq!(location("/srv/git/repo.git"), None);
        // GitHub's and GitLab's SSH over port 443.
        assert_eq!(location("ssh://git@ssh.github.com:443/owner/repo.git"), gh);
        assert_eq!(
            location("ssh://git@altssh.gitlab.com:443/g/p.git"),
            Some(("gitlab.com".to_owned(), "g/p".to_owned()))
        );
    }

    #[test]
    fn web_pages_of_remotes() {
        let page = Some("https://github.com/JKamsker/gitcraft".to_owned());
        assert_eq!(web_url("https://github.com/JKamsker/gitcraft.git"), page);
        assert_eq!(web_url("git@github.com:JKamsker/gitcraft.git"), page);
        assert_eq!(web_url("ssh://git@github.com:22/JKamsker/gitcraft"), page);
        assert_eq!(
            web_url("http://git.example.org/team/repo.git"),
            Some("http://git.example.org/team/repo".to_owned())
        );
        assert_eq!(
            web_url("https://git.example.org:8443/team/repo.git"),
            Some("https://git.example.org:8443/team/repo".to_owned())
        );
        assert_eq!(
            web_url("ssh://git@git.example.org:2222/team/repo.git"),
            Some("https://git.example.org/team/repo".to_owned())
        );
        assert_eq!(web_url("C:/repos/gitcraft"), None);
        assert_eq!(web_url("/srv/git/repo.git"), None);
    }

    #[test]
    fn github_avatars_by_email() {
        let json = r#"[
            {"commit": {"author": {"email": "Ann@x.org"}, "committer": {"email": "noreply@github.com"}},
             "author": {"avatar_url": "https://a/1?v=4"}, "committer": {"avatar_url": "https://a/web?v=4"}},
            {"commit": {"author": {"email": "nobody@x.org"}, "committer": {"email": "nobody@x.org"}},
             "author": null, "committer": null}
        ]"#;
        let m = parse_github_avatars(json);
        assert_eq!(
            m.get("ann@x.org").map(String::as_str),
            Some("https://a/1?v=4")
        );
        assert!(m.contains_key("noreply@github.com"));
        assert!(!m.contains_key("nobody@x.org"));
    }

    #[test]
    fn detects_forge_from_origin() {
        let r = |name: &str, url: &str| Remote {
            name: name.into(),
            url: url.into(),
        };
        assert_eq!(
            detect(&[r("origin", "git@github.com:a/b.git")]),
            Some(Forge::GitHub)
        );
        assert_eq!(
            detect(&[
                r("up", "https://github.com/x/y"),
                r("origin", "https://gitlab.com/a/b")
            ]),
            Some(Forge::GitLab)
        );
        assert_eq!(detect(&[r("origin", "C:/repos/bare.git")]), None);
        // A folder's name is no forge.
        assert_eq!(detect(&[r("origin", "/srv/gitlab-backup/x.git")]), None);
        assert_eq!(detect(&[r("origin", "C:/gitlab/x.git")]), None);
        assert_eq!(
            detect(&[r("origin", "https://github.example.com/a/b")]),
            Some(Forge::GitHub)
        );
    }

    #[test]
    fn only_the_host_tells_the_forge() {
        let r = |url: &str| Remote {
            name: "origin".into(),
            url: url.into(),
        };
        assert_eq!(
            detect(&[r("git@gitlab.com:me/me.github.io.git")]),
            Some(Forge::GitLab)
        );
        assert_eq!(detect(&[r("https://git.example.org/github.com/x")]), None);
        assert_eq!(detect(&[r("https://git.example.org/a@github.com/x")]), None);
        assert_eq!(
            detect(&[r("https://github-mirror.gitlab.example.org/g/p.git")]),
            Some(Forge::GitLab)
        );
        assert_eq!(
            location("https://me:tok@github.com/a/b.git"),
            Some(("github.com".into(), "a/b".into()))
        );
        assert_eq!(
            detect(&[r("https://example.org/gitlab/x/y")]),
            Some(Forge::GitLab)
        );
    }

    #[test]
    fn an_app_opening_a_pull_request_goes_by_its_name_alone() {
        let json = r#"[{"number": 1, "title": "t", "headRefName": "b",
            "author": {"login": "app/github-actions"}, "url": "u", "isDraft": false}]"#;
        let list = parse(Forge::GitHub, json);
        assert_eq!(
            list.map(|l| l[0].author.clone()).as_deref(),
            Ok("github-actions")
        );
    }

    #[test]
    fn parses_both_forges() {
        let gh = r#"[{"number":7,"title":"Fix","headRefName":"fix/x","headRefOid":"c0ffee","author":{"login":"ana"},"url":"https://g/7","isDraft":true}]"#;
        let prs = parse(Forge::GitHub, gh).unwrap_or_default();
        assert_eq!(prs.len(), 1);
        assert_eq!(
            (prs[0].number, prs[0].branch.as_str(), prs[0].draft),
            (7, "fix/x", true)
        );
        let gl = r#"[{"iid":3,"title":"Add","source_branch":"feat","sha":"beef","author":{"username":"bo"},"web_url":"u","draft":false}]"#;
        let mrs = parse(Forge::GitLab, gl).unwrap_or_default();
        assert_eq!((mrs[0].number, mrs[0].author.as_str()), (3, "bo"));
        assert_eq!(
            (prs[0].sha.as_str(), mrs[0].sha.as_str()),
            ("c0ffee", "beef")
        );
    }
}
