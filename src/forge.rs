//! Pull requests through the forges' own command-line tools: `gh` for GitHub, `glab` for
//! GitLab. They hold the user's login, so gitcraft never stores a token.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

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
    let url = remote.url.to_lowercase();
    if url.contains("github.com") || url.contains("github.") {
        Some(Forge::GitHub)
    } else if url.contains("gitlab") {
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
    pub author: String,
    pub url: String,
    pub draft: bool,
}

pub(crate) fn tool(forge: Forge, repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new(forge.tool());
    cmd.current_dir(repo).args(args);
    cmd.env("GH_PROMPT_DISABLED", "1").env("NO_PROMPT", "1");
    crate::shell_path::apply(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|_| {
        format!(
            "{} isn't installed: gitcraft uses it for {}s",
            forge.tool(),
            forge.noun()
        )
    })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
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
                "number,title,headRefName,author,url,isDraft",
            ],
        )?,
        Forge::GitLab => tool(forge, repo, &["mr", "list", "--output", "json"])?,
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
                author: s(item, &["author", "login"]),
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
            let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
            let (host, path) = rest.split_once('/')?;
            (host.split(':').next().unwrap_or(host), path)
        }
        None => {
            let rest = url.rsplit_once('@').map_or(url, |(_, r)| r);
            rest.split_once(':')?
        }
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && path.contains('/')).then(|| (host.to_lowercase(), path.to_owned()))
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
    }

    #[test]
    fn parses_both_forges() {
        let gh = r#"[{"number":7,"title":"Fix","headRefName":"fix/x","author":{"login":"ana"},"url":"https://g/7","isDraft":true}]"#;
        let prs = parse(Forge::GitHub, gh).unwrap_or_default();
        assert_eq!(prs.len(), 1);
        assert_eq!(
            (prs[0].number, prs[0].branch.as_str(), prs[0].draft),
            (7, "fix/x", true)
        );
        let gl = r#"[{"iid":3,"title":"Add","source_branch":"feat","author":{"username":"bo"},"web_url":"u","draft":false}]"#;
        let mrs = parse(Forge::GitLab, gl).unwrap_or_default();
        assert_eq!((mrs[0].number, mrs[0].author.as_str()), (3, "bo"));
    }
}
