//! Git flow (feature / release / hotfix branches) done with plain git commands, reading and
//! writing the same `gitflow.*` config as git-flow (AVH / next), so both can be used on a
//! repository. Nothing extra has to be installed.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Set up (`gitflow.branch.develop` is set).
    pub initialized: bool,
    /// Production branch (`gitflow.branch.master`).
    pub main: String,
    pub develop: String,
    pub feature: String,
    pub release: String,
    pub hotfix: String,
    /// Prefix of release / hotfix tags (`gitflow.prefix.versiontag`), often empty or `v`.
    pub tag: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            initialized: false,
            main: "main".to_owned(),
            develop: "develop".to_owned(),
            feature: "feature/".to_owned(),
            release: "release/".to_owned(),
            hotfix: "hotfix/".to_owned(),
            tag: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Feature,
    Release,
    Hotfix,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Feature, Kind::Release, Kind::Hotfix];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Feature => "feature",
            Kind::Release => "release",
            Kind::Hotfix => "hotfix",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Kind::Feature => "Feature",
            Kind::Release => "Release",
            Kind::Hotfix => "Hotfix",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Kind::Feature => "git-branch",
            Kind::Release => "package",
            Kind::Hotfix => "wrench",
        }
    }

    /// What its name is: a version for releases and hotfixes.
    pub fn hint(self) -> &'static str {
        match self {
            Kind::Feature => "my-feature",
            Kind::Release => "1.2.0",
            Kind::Hotfix => "1.2.1",
        }
    }

    pub fn prefix(self, c: &Config) -> &str {
        match self {
            Kind::Feature => &c.feature,
            Kind::Release => &c.release,
            Kind::Hotfix => &c.hotfix,
        }
    }

    /// The branch it starts from.
    pub fn base(self, c: &Config) -> &str {
        match self {
            Kind::Feature | Kind::Release => &c.develop,
            Kind::Hotfix => &c.main,
        }
    }
}

impl Config {
    /// The kind and name of a flow branch: `feature/login` → (Feature, `login`).
    pub fn classify<'a>(&self, branch: &'a str) -> Option<(Kind, &'a str)> {
        if !self.initialized {
            return None;
        }
        Kind::ALL.into_iter().find_map(|k| {
            let p = k.prefix(self);
            if p.is_empty() {
                return None;
            }
            branch
                .strip_prefix(p)
                .filter(|n| !n.is_empty())
                .map(|n| (k, n))
        })
    }
}

/// Reads `gitflow.*`; `locals`: the local branch names, to guess the production branch of a
/// repository not set up yet.
pub fn config(repo: &Path, locals: &[String]) -> Config {
    let out = crate::git::run_ok_codes(
        Some(repo),
        &["config", "--get-regexp", r"^gitflow\."],
        &[0, 1],
    )
    .unwrap_or_default();
    parse(&out, locals)
}

fn parse(out: &str, locals: &[String]) -> Config {
    let mut c = Config::default();
    if locals.iter().all(|b| b != "main") && locals.iter().any(|b| b == "master") {
        c.main = "master".to_owned();
    }
    for line in out.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let value = value.trim().to_owned();
        match key {
            "gitflow.branch.master" | "gitflow.branch.main" if !value.is_empty() => c.main = value,
            "gitflow.branch.develop" if !value.is_empty() => {
                c.develop = value;
                c.initialized = true;
            }
            "gitflow.prefix.feature" => c.feature = value,
            "gitflow.prefix.release" => c.release = value,
            "gitflow.prefix.hotfix" => c.hotfix = value,
            "gitflow.prefix.versiontag" => c.tag = value,
            _ => {}
        }
    }
    c
}

fn cmd(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

/// Writes the config (git-flow's keys) and creates the develop branch from the production
/// one when it's missing.
pub fn init_ops(c: &Config, locals: &[String]) -> Vec<Vec<String>> {
    let mut ops = vec![
        cmd(&["config", "gitflow.branch.master", &c.main]),
        cmd(&["config", "gitflow.branch.develop", &c.develop]),
        cmd(&["config", "gitflow.prefix.feature", &c.feature]),
        cmd(&["config", "gitflow.prefix.release", &c.release]),
        cmd(&["config", "gitflow.prefix.hotfix", &c.hotfix]),
        cmd(&["config", "gitflow.prefix.support", "support/"]),
        cmd(&["config", "gitflow.prefix.bugfix", "bugfix/"]),
        cmd(&["config", "gitflow.prefix.versiontag", &c.tag]),
    ];
    if !locals.contains(&c.develop) {
        ops.push(cmd(&["branch", &c.develop, &c.main]));
    }
    ops
}

/// Creates and checks out the branch `kind`/`name` from its base.
pub fn start_ops(c: &Config, kind: Kind, name: &str) -> Vec<Vec<String>> {
    let branch = format!("{}{name}", kind.prefix(c));
    vec![cmd(&["switch", "-c", &branch, kind.base(c)])]
}

/// Merges the branch where it belongs and deletes it: a feature into develop; a release or
/// hotfix into the production branch, tagged, then into develop. Runs again cleanly after a
/// conflict was resolved and committed (merging what's merged does nothing; an existing tag is
/// kept).
pub fn finish_ops(
    c: &Config,
    kind: Kind,
    name: &str,
    tag_exists: bool,
    delete: bool,
) -> Vec<Vec<String>> {
    let branch = format!("{}{name}", kind.prefix(c));
    let merge = |into: &str| {
        let msg = format!("Merge branch '{branch}' into {into}");
        vec![
            cmd(&["switch", into]),
            cmd(&["merge", "--no-ff", "-m", &msg, &branch]),
        ]
    };
    let mut ops = Vec::new();
    match kind {
        Kind::Feature => ops.extend(merge(&c.develop)),
        Kind::Release | Kind::Hotfix => {
            ops.extend(merge(&c.main));
            if !tag_exists {
                let tag = format!("{}{name}", c.tag);
                let msg = format!("{} {name}", kind.title());
                ops.push(cmd(&["tag", "-a", &tag, "-m", &msg]));
            }
            ops.extend(merge(&c.develop));
        }
    }
    if delete {
        ops.push(cmd(&["branch", "-d", &branch]));
    }
    ops
}

/// The tag a release or hotfix finish creates.
pub fn tag_name(c: &Config, name: &str) -> String {
    format!("{}{name}", c.tag)
}

/// The commands of `ops` as the user would type them, for a confirmation.
pub fn describe(ops: &[Vec<String>]) -> String {
    ops.iter()
        .map(|op| {
            let words: Vec<String> = op
                .iter()
                .map(|w| {
                    if w.contains(' ') {
                        format!("\"{w}\"")
                    } else {
                        w.clone()
                    }
                })
                .collect();
            format!("git {}", words.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locals(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn reads_git_flow_config() {
        let out = "gitflow.branch.master production\ngitflow.branch.develop dev\ngitflow.prefix.feature feat/\ngitflow.prefix.versiontag v\n";
        let c = parse(out, &locals(&["main"]));
        assert!(c.initialized);
        assert_eq!((c.main.as_str(), c.develop.as_str()), ("production", "dev"));
        assert_eq!((c.feature.as_str(), c.tag.as_str()), ("feat/", "v"));
        assert_eq!(c.release, "release/");
        assert_eq!(c.classify("feat/login"), Some((Kind::Feature, "login")));
        assert_eq!(c.classify("release/1.0"), Some((Kind::Release, "1.0")));
        assert_eq!(c.classify("feat/"), None);
        assert_eq!(c.classify("dev"), None);
    }

    #[test]
    fn guesses_master_before_set_up() {
        let c = parse("", &locals(&["master", "x"]));
        assert!(!c.initialized);
        assert_eq!(c.main, "master");
        assert_eq!(parse("", &locals(&["main", "master"])).main, "main");
        assert_eq!(c.classify("feature/x"), None);
    }

    #[test]
    fn init_creates_develop_only_when_missing() {
        let c = Config::default();
        let with = init_ops(&c, &locals(&["main"]));
        assert_eq!(with.last(), Some(&cmd(&["branch", "develop", "main"])));
        let without = init_ops(&c, &locals(&["main", "develop"]));
        assert!(without.iter().all(|op| op[0] == "config"));
    }

    #[test]
    fn start_and_finish() {
        let c = Config {
            initialized: true,
            tag: "v".to_owned(),
            ..Config::default()
        };
        assert_eq!(
            start_ops(&c, Kind::Hotfix, "1.0.1"),
            vec![cmd(&["switch", "-c", "hotfix/1.0.1", "main"])]
        );
        let feature = finish_ops(&c, Kind::Feature, "x", false, true);
        assert_eq!(
            feature,
            vec![
                cmd(&["switch", "develop"]),
                cmd(&[
                    "merge",
                    "--no-ff",
                    "-m",
                    "Merge branch 'feature/x' into develop",
                    "feature/x"
                ]),
                cmd(&["branch", "-d", "feature/x"]),
            ]
        );
        let release = finish_ops(&c, Kind::Release, "2.0", false, false);
        let firsts: Vec<&str> = release.iter().map(|op| op[0].as_str()).collect();
        assert_eq!(firsts, ["switch", "merge", "tag", "switch", "merge"]);
        assert_eq!(release[1].last().map(String::as_str), Some("release/2.0"));
        assert_eq!(release[2][2], "v2.0");
        assert_eq!(release[3][1], "develop");
        let again = finish_ops(&c, Kind::Release, "2.0", true, false);
        assert!(again.iter().all(|op| op[0] != "tag"));
    }

    #[test]
    fn describes_commands() {
        let ops = vec![cmd(&["merge", "-m", "a b", "x"])];
        assert_eq!(describe(&ops), "git merge -m \"a b\" x");
    }
}
