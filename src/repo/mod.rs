//! One open repository: its state, background git work and the view
//! (branches on the left, commit graph or diff in the middle, commit detail / changes on the right).

mod conflict;
mod detail;
mod dialogs;
mod diff;
mod file_view;
mod graph_view;
mod highlight;
mod rebase;
mod sidebar;
pub(crate) mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::{Key, Ui};

use crate::forge::{self, Forge, PullRequest};
use crate::git::{
    self, Commit, CommitDetail, Identity, InProgress, Refs, Remote, Stash, StatusFile, WorkStatus,
    Worktree,
};
use crate::graph::{self, Graph};
use crate::icons;
use crate::settings::Settings;
use crate::terminal::{ShellSpec, Terminal};
use crate::theme::Tokens;

pub use dialogs::Dialog;
pub use widgets::primary_button;

/// Hash of the pseudo-commit that stands for uncommitted changes at the top of the graph.
pub(crate) const WIP_HASH: &str = "\u{0}wip";
/// History deeper than this isn't loaded by scrolling or searching.
const MAX_HISTORY: usize = 100_000;
/// An operation that takes longer than this gets a notification when it ends.
const LONG_OP: Duration = Duration::from_secs(4);

/// Results coming back from background git calls.
enum Msg {
    Loaded(Result<Box<Loaded>, String>),
    More(Result<Vec<Commit>, String>),
    Status(Result<WorkStatus, String>),
    Detail(String, Result<CommitDetail, String>),
    Diff(DiffKey, Result<diff::DiffData, String>),
    FileHistory(String, Result<Vec<git::FileCommit>, String>),
    Blame(String, Option<String>, Result<file_view::BlameData, String>),
    RebaseCommits {
        onto: String,
        label: String,
        result: Result<Vec<git::RebaseCommit>, String>,
    },
    Search(String, Result<Vec<String>, String>),
    /// A comparison's files, for the request numbered so.
    Compare(u64, Result<CompareData, String>),
    Prs(Result<Vec<PullRequest>, String>),
    /// The commits to replay when rewording `hash`.
    RewordCommits {
        hash: String,
        message: String,
        result: Result<Vec<git::RebaseCommit>, String>,
    },
    OpDone {
        label: String,
        result: Result<String, String>,
        refresh: Refresh,
        kind: OpKind,
        quiet: bool,
    },
}

struct Loaded {
    refs: Refs,
    history: Vec<Commit>,
    work: WorkStatus,
    worktrees: Vec<Worktree>,
    stashes: Vec<Stash>,
    remotes: Vec<Remote>,
    identity: Identity,
    /// How many commits were asked for (more exist when that many came back).
    requested: usize,
}

/// What to re-read after an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refresh {
    /// Only the working tree (staging changes nothing else).
    Status,
    All,
}

/// How an operation can be undone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Undo {
    None,
    /// The operation moved HEAD (commit, merge, reset…): undo moves it back with
    /// `git reset <mode>`, redo forward again. `--soft` keeps the changes staged.
    HeadMove {
        mode: &'static str,
    },
    /// Explicit commands each way (branch create/delete/rename, checkout, tags, stash).
    Custom {
        undo: Vec<Vec<String>>,
        redo: Vec<Vec<String>>,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct UndoEntry {
    label: String,
    undo: Vec<Vec<String>>,
    redo: Vec<Vec<String>>,
}

/// An operation as `run_ops_ex` got it, kept to retry it with credentials.
struct PendingOp {
    label: String,
    ops: Vec<Vec<String>>,
    refresh: Refresh,
    kind: OpKind,
    quiet: bool,
}

#[derive(Clone, Debug)]
enum OpKind {
    Normal(Undo, String),
    Undo(UndoEntry),
    Redo(UndoEntry),
}

/// Which diff the central area shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DiffKey {
    Commit {
        hash: String,
        path: String,
    },
    Work(StatusFile),
    /// A file from one commit to another, or to the working tree (`to: None`).
    Range {
        from: String,
        to: Option<String>,
        path: String,
    },
}

pub(crate) struct DiffView {
    key: DiffKey,
    data: Option<Result<diff::DiffData, String>>,
    /// Changed lines clicked, to stage, unstage or discard only those.
    chosen: std::collections::BTreeSet<usize>,
    /// The last line clicked, where a Shift+click range starts.
    anchor: Option<usize>,
}

impl DiffView {
    fn new(key: DiffKey) -> Self {
        Self {
            key,
            data: None,
            chosen: Default::default(),
            anchor: None,
        }
    }
}

/// A commit's message being edited in the right pane (double-click on it).
pub(crate) struct Reword {
    pub hash: String,
    pub subject: String,
    pub body: String,
    /// Put the cursor in the summary on the next frame.
    pub focus: bool,
}

/// Two points of history side by side in the right pane: a branch against another, a commit
/// against the working tree…
pub(crate) struct Compare {
    base: String,
    base_label: String,
    /// `None`: the working tree.
    target: Option<String>,
    target_label: String,
    /// Only what the target changed since the two diverged (from their merge base), like a pull
    /// request shows it.
    since_fork: bool,
    data: Option<Result<CompareData, String>>,
    /// Numbers the requests, so an older answer doesn't replace a newer one.
    request: u64,
}

pub(crate) struct CompareData {
    /// Where the diff starts: the base's commit, or the merge base.
    from: String,
    /// The target's commit (`None`: the working tree).
    to: Option<String>,
    files: Vec<git::ChangedFile>,
    /// Commits only the base has, and only the target (HEAD for the working tree) has.
    apart: (u32, u32),
}

impl Compare {
    /// How the comparison calls one of its two ends.
    fn name_of(&self, rev: &str) -> Option<String> {
        let data = self.data.as_ref()?.as_ref().ok()?;
        if data.to.as_deref() == Some(rev) {
            Some(self.target_label.clone())
        } else if data.from == rev && self.since_fork {
            Some(format!("merge base {}", short(rev)))
        } else if data.from == rev {
            Some(self.base_label.clone())
        } else {
            None
        }
    }
}

#[derive(Default)]
struct Search {
    open: bool,
    focus: bool,
    /// Search what commits added or removed in files (`git log -S`), not their messages.
    code: bool,
    query: String,
    /// The query the results are for.
    searched: String,
    results: Vec<String>,
    matches: HashSet<String>,
    current: usize,
    running: bool,
    edited: Option<Instant>,
}

/// Everything the views ask for; applied after drawing so views only borrow the state.
pub(crate) enum Action {
    Select(usize),
    SelectHash(String),
    SelectStash(usize),
    OpenDiff(DiffKey),
    CloseDiff,
    Ops {
        label: String,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
    },
    UndoableOps {
        label: String,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
        undo: Undo,
    },
    Checkout(String),
    /// Checks out a local branch behind its upstream, then fast-forwards it.
    CheckoutPull(String),
    CheckoutRemote(String),
    Dialog(Dialog),
    Commit,
    Fetch,
    Pull,
    Push,
    Reload,
    LoadMore,
    Undo,
    Redo,
    StashPush(String),
    /// Open a folder (a worktree) in its own tab.
    OpenTab(PathBuf),
    ToggleTerminal,
    NewTerminal {
        shell: Option<ShellSpec>,
        cwd: Option<PathBuf>,
    },
    CloseTerminal(usize),
    SelectTerminal(usize),
    ToggleSearch,
    SearchStep(i32),
    CreatePr {
        branch: String,
        base: String,
        title: String,
        body: String,
        draft: bool,
    },
    OpenUrl(String),
    /// Open a path (relative to the repository) with the system's default app.
    OpenPath(PathBuf),
    OpenInEditor(PathBuf),
    SetIdentity(String, String),
    SetDiffOptions {
        split: bool,
        ignore_ws: bool,
    },
    SaveConflict,
    /// A file's history or blame in the central pane (`rev`: the commit to blame at).
    FileView {
        path: String,
        rev: Option<String>,
        tab: file_view::FileTab,
    },
    CloseFileView,
    /// Open the interactive rebase editor for the commits after `onto`.
    /// Gives a commit of the checked-out branch a new message: amending the latest, rebasing
    /// for an older one.
    Reword {
        hash: String,
        message: String,
    },
    InteractiveRebase {
        onto: String,
        label: String,
    },
    RunRebase,
    CloseRebase,
    /// Remember these HTTPS credentials and run the operation that needed them again.
    SignIn(git::Credentials),
    /// Add `pattern` to `.gitignore`; stop tracking `path` if git tracks it.
    Ignore {
        pattern: String,
        path: String,
    },
    /// Stage, unstage or discard these changed lines of the open diff.
    PickLines {
        pick: diff::Pick,
        lines: std::collections::BTreeSet<usize>,
    },
    /// Compare `base` with `target` (the working tree when `None`) in the right pane.
    Compare {
        base: String,
        base_label: String,
        target: Option<String>,
        target_label: String,
        since_fork: bool,
    },
    /// Show only the target's changes since the two diverged, or every difference.
    CompareSinceFork(bool),
    SwapCompare,
    CloseCompare,
    /// Search commit messages, or what commits changed in files.
    SearchCode(bool),
}

pub struct RepoTab {
    pub path: PathBuf,
    pub name: String,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    loading: bool,
    load_error: Option<String>,
    refs: Refs,
    /// Commits as git listed them.
    history: Vec<Commit>,
    has_more: bool,
    loading_more: bool,
    work: WorkStatus,
    /// Rows of the graph: the WIP pseudo-commit (when the tree is dirty) followed by `history`.
    commits: Vec<Commit>,
    graph: Graph,
    index: HashMap<String, usize>,
    worktrees: Vec<Worktree>,
    stashes: Vec<Stash>,
    remotes: Vec<Remote>,
    identity: Identity,
    forge: Option<Forge>,
    prs: Vec<PullRequest>,
    pr_error: Option<String>,
    prs_loaded: Option<Instant>,

    selected: Option<usize>,
    /// A stash shown in the detail pane instead of a graph row.
    stash_view: Option<usize>,
    scroll_to: Option<usize>,
    /// The commit the detail pane is about (a graph row or a stash).
    detail_hash: Option<String>,
    detail: Option<CommitDetail>,
    detail_error: Option<String>,
    diff: Option<DiffView>,
    compare: Option<Compare>,
    compare_requests: u64,
    file_view: Option<file_view::FileView>,
    rebase: Option<rebase::RebaseEditor>,
    /// The selected commit's message, being edited in the right pane.
    reword: Option<Reword>,
    /// HTTPS credentials typed in this session, used for every later git command of the tab.
    credentials: Option<git::Credentials>,
    /// The last operation started, to run again after signing in.
    last_op: Option<PendingOp>,
    /// The forge has been asked for avatars.
    avatars_hinted: bool,
    diff_split: bool,
    diff_ignore_ws: bool,
    conflict: Option<conflict::ConflictEditor>,

    commit_summary: String,
    commit_body: String,
    amend: bool,
    dialog: Option<Dialog>,
    search: Search,
    /// A commit to select once enough history is loaded (search results, links).
    pending_goto: Option<String>,

    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,

    busy_op: Option<String>,
    op_started: Option<Instant>,
    status: Option<(String, bool)>,
    branch_filter: String,
    /// Jump to the changes row on the next rebuild (after an operation left conflicts).
    focus_wip: bool,
    last_fetch: Instant,

    terminals: Vec<Terminal>,
    active_terminal: usize,
    terminal_open: bool,
    terminal_error: Option<String>,

    watcher: Option<notify::RecommendedWatcher>,
    fs_rx: Option<Receiver<bool>>,
    /// A change on disk not yet reloaded: (inside .git, when).
    fs_pending: Option<(bool, Instant)>,

    /// Actions queued from outside (the command palette).
    queued: Vec<Action>,
    /// A folder the app should open in a new tab (taken by `App`).
    pub open_request: Option<PathBuf>,
    /// The app has a dialog of its own open (palette, settings): leave the keys to it.
    pub keys_blocked: bool,
    /// A long operation that just ended: (message, failed), for a notification (taken by `App`).
    pub finished: Option<(String, bool)>,
}

impl RepoTab {
    pub fn new(path: PathBuf, ctx: &egui::Context) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let (tx, rx) = channel();
        let mut tab = Self {
            path,
            name,
            tx,
            rx,
            loading: false,
            load_error: None,
            refs: Refs::default(),
            history: Vec::new(),
            has_more: false,
            loading_more: false,
            work: WorkStatus::default(),
            commits: Vec::new(),
            graph: Graph {
                rows: Vec::new(),
                width: 0,
            },
            index: HashMap::new(),
            worktrees: Vec::new(),
            stashes: Vec::new(),
            remotes: Vec::new(),
            identity: Identity::default(),
            forge: None,
            prs: Vec::new(),
            pr_error: None,
            prs_loaded: None,
            selected: None,
            stash_view: None,
            scroll_to: None,
            detail_hash: None,
            detail: None,
            detail_error: None,
            diff: None,
            compare: None,
            compare_requests: 0,
            file_view: None,
            rebase: None,
            reword: None,
            credentials: None,
            last_op: None,
            avatars_hinted: false,
            diff_split: false,
            diff_ignore_ws: false,
            conflict: None,
            commit_summary: String::new(),
            commit_body: String::new(),
            amend: false,
            dialog: None,
            search: Search::default(),
            pending_goto: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            busy_op: None,
            op_started: None,
            status: None,
            branch_filter: String::new(),
            focus_wip: false,
            last_fetch: Instant::now(),
            terminals: Vec::new(),
            active_terminal: 0,
            terminal_open: false,
            terminal_error: None,
            watcher: None,
            fs_rx: None,
            fs_pending: None,
            queued: Vec::new(),
            open_request: None,
            keys_blocked: false,
            finished: None,
        };
        tab.watch(ctx);
        tab.reload(ctx);
        tab
    }

    pub fn is_busy(&self) -> bool {
        self.loading || self.busy_op.is_some()
    }

    /// Queues an action from outside the tab (the command palette).
    pub(crate) fn queue(&mut self, action: Action) {
        self.queued.push(action);
    }

    /// Re-reads refs, history, working tree, worktrees, stashes and remotes in the background.
    pub fn reload(&mut self, ctx: &egui::Context) {
        if self.loading {
            return;
        }
        self.loading = true;
        let path = self.path.clone();
        // Keep the pages already loaded.
        let count = self.history.len().clamp(git::LOG_PAGE, MAX_HISTORY);
        spawn(&self.tx, ctx, move || {
            Msg::Loaded((|| {
                let refs = git::refs(&path)?;
                let history = git::log(&path, 0, count)?;
                let work = git::status(&path)?;
                // Very old gits have no `worktree list`; the section just stays empty.
                let worktrees = git::worktrees(&path).unwrap_or_default();
                let stashes = git::stashes(&path).unwrap_or_default();
                let remotes = git::remotes(&path).unwrap_or_default();
                let identity = git::identity(&path);
                Ok(Box::new(Loaded {
                    refs,
                    history,
                    work,
                    worktrees,
                    stashes,
                    remotes,
                    identity,
                    requested: count,
                }))
            })())
        });
    }

    fn reload_status(&mut self, ctx: &egui::Context) {
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || Msg::Status(git::status(&path)));
    }

    fn load_more(&mut self, ctx: &egui::Context) {
        if self.loading_more || !self.has_more || self.history.len() >= MAX_HISTORY {
            return;
        }
        self.loading_more = true;
        let path = self.path.clone();
        let skip = self.history.len();
        spawn(&self.tx, ctx, move || {
            Msg::More(git::log(&path, skip, git::LOG_PAGE))
        });
    }

    fn load_prs(&mut self, ctx: &egui::Context) {
        let Some(forge) = self.forge else { return };
        self.prs_loaded = Some(Instant::now());
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || Msg::Prs(forge::list(&path, forge)));
    }

    /// Watches the folder so changes made elsewhere (editor, terminal) show up by themselves.
    fn watch(&mut self, ctx: &egui::Context) {
        use notify::Watcher;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            for path in &event.paths {
                let s = path.to_string_lossy().replace('\\', "/");
                match s.split_once("/.git/") {
                    // Refs, HEAD, the index and merge state matter; objects and logs don't.
                    Some((_, inside)) => {
                        let relevant = inside.starts_with("refs")
                            || inside == "HEAD"
                            || inside == "index"
                            || inside == "packed-refs"
                            || inside.ends_with("_HEAD")
                            || inside.starts_with("rebase-");
                        if relevant {
                            let _ = tx.send(true);
                            ctx.request_repaint_after(Duration::from_millis(500));
                        }
                    }
                    None => {
                        let _ = tx.send(false);
                        ctx.request_repaint_after(Duration::from_millis(500));
                    }
                }
            }
        };
        if let Ok(mut watcher) = notify::recommended_watcher(handler)
            && watcher
                .watch(&self.path, notify::RecursiveMode::Recursive)
                .is_ok()
        {
            self.watcher = Some(watcher);
            self.fs_rx = Some(rx);
        }
    }

    fn is_wip(&self, row: usize) -> bool {
        self.commits.get(row).is_some_and(|c| c.hash == WIP_HASH)
    }

    fn selected_hash(&self) -> Option<String> {
        self.selected
            .and_then(|r| self.commits.get(r))
            .map(|c| c.hash.clone())
    }

    /// Rebuilds the graph rows from `history` and `work`, keeping the selection.
    fn rebuild(&mut self, ctx: &egui::Context) {
        let previous = self.selected_hash();
        self.commits.clear();
        if !self.work.is_clean() {
            self.commits.push(Commit {
                hash: WIP_HASH.to_owned(),
                parents: if self.refs.head.is_empty() {
                    Vec::new()
                } else {
                    vec![self.refs.head.clone()]
                },
                time: git::now(),
                ..Commit::default()
            });
        }
        self.commits.extend(self.history.iter().cloned());
        self.graph = graph::layout(&self.commits);
        self.index = self
            .commits
            .iter()
            .enumerate()
            .map(|(i, c)| (c.hash.clone(), i))
            .collect();

        // Keep the selection; otherwise start on the changes, or HEAD.
        let mut keep = previous.as_ref().and_then(|h| self.index.get(h).copied());
        if std::mem::take(&mut self.focus_wip) && self.index.contains_key(WIP_HASH) {
            keep = None;
            self.stash_view = None;
        }
        if self.stash_view.is_none() {
            let target = keep
                .or_else(|| self.index.get(WIP_HASH).copied())
                .or_else(|| self.index.get(&self.refs.head).copied());
            match target {
                Some(row) if keep == Some(row) => self.selected = Some(row),
                Some(row) => {
                    self.selected = None;
                    self.select(row, ctx);
                    self.scroll_to = Some(row);
                }
                None => {
                    self.selected = None;
                    self.detail = None;
                }
            }
        }

        // A merge or cherry-pick leaves a message to commit with.
        if let Some(msg) = &self.work.prepared_message
            && self.commit_summary.is_empty()
            && self.commit_body.is_empty()
        {
            let (summary, body) = msg.split_once('\n').unwrap_or((msg.as_str(), ""));
            self.commit_summary = summary.trim().to_owned();
            self.commit_body = body.trim().to_owned();
        }

        // The diff of a file that's no longer listed (staged, committed…) is stale.
        if let Some(DiffView {
            key: DiffKey::Work(file),
            ..
        }) = &self.diff
        {
            let still = self
                .work
                .staged
                .iter()
                .chain(&self.work.unstaged)
                .chain(&self.work.conflicted)
                .any(|f| f == file);
            let key = DiffKey::Work(file.clone());
            if still && file.area != git::Area::Conflicted {
                // Shown as it was until the new diff is read (no flicker after staging lines).
                self.refresh_diff(ctx);
            } else {
                self.diff = None;
                self.conflict = None;
                if still {
                    self.open_diff(key, ctx);
                }
            }
        }

        if let Some(hash) = self.pending_goto.clone() {
            self.goto(&hash, ctx);
        }
    }

    fn select(&mut self, row: usize, ctx: &egui::Context) {
        let Some(commit) = self.commits.get(row) else {
            return;
        };
        if self.selected == Some(row) && self.stash_view.is_none() {
            return;
        }
        let hash = commit.hash.clone();
        self.selected = Some(row);
        self.stash_view = None;
        self.file_view = None;
        self.compare = None;
        if matches!(
            self.diff,
            Some(DiffView {
                key: DiffKey::Commit { .. } | DiffKey::Range { .. },
                ..
            })
        ) || hash != WIP_HASH
        {
            self.diff = None;
            self.conflict = None;
        }
        if hash == WIP_HASH {
            self.detail = None;
            self.detail_hash = None;
            return;
        }
        self.load_detail(hash, ctx);
    }

    fn load_detail(&mut self, hash: String, ctx: &egui::Context) {
        self.detail = None;
        self.reword = None;
        self.detail_error = None;
        self.detail_hash = Some(hash.clone());
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || {
            let detail = git::commit_detail(&path, &hash);
            Msg::Detail(hash, detail)
        });
    }

    fn select_stash(&mut self, i: usize, ctx: &egui::Context) {
        let Some(stash) = self.stashes.get(i) else {
            return;
        };
        let hash = stash.hash.clone();
        self.stash_view = Some(i);
        self.selected = None;
        self.file_view = None;
        self.compare = None;
        self.diff = None;
        self.conflict = None;
        self.load_detail(hash, ctx);
    }

    fn select_hash(&mut self, hash: &str, ctx: &egui::Context) {
        match self.index.get(hash).copied() {
            Some(row) => {
                self.select(row, ctx);
                self.scroll_to = Some(row);
            }
            None => {
                self.status = Some((
                    format!(
                        "Commit {} is not in the loaded history",
                        hash.get(..8).unwrap_or(hash)
                    ),
                    true,
                ));
            }
        }
    }

    /// Selects `hash`, loading more history first if it's further down.
    fn goto(&mut self, hash: &str, ctx: &egui::Context) {
        if self.index.contains_key(hash) {
            self.pending_goto = None;
            self.select_hash(hash, ctx);
        } else if self.has_more && self.history.len() < MAX_HISTORY {
            self.pending_goto = Some(hash.to_owned());
            self.load_more(ctx);
        } else {
            self.pending_goto = None;
            self.select_hash(hash, ctx);
        }
    }

    /// Shows a diff (or the conflict editor) in the central area; asking for the open one
    /// closes it.
    fn open_diff(&mut self, key: DiffKey, ctx: &egui::Context) {
        if self.diff.as_ref().is_some_and(|d| d.key == key) {
            self.diff = None;
            self.conflict = None;
            return;
        }
        self.conflict = match &key {
            DiffKey::Work(file) if file.area == git::Area::Conflicted => {
                conflict::ConflictEditor::open(&self.path, &file.path, self.work.in_progress)
            }
            _ => None,
        };
        self.diff = Some(DiffView::new(key));
        self.refresh_diff(ctx);
    }

    /// Reads the open diff again, showing the old one meanwhile.
    fn refresh_diff(&mut self, ctx: &egui::Context) {
        let Some(view) = &self.diff else { return };
        let key = view.key.clone();
        let path = self.path.clone();
        let ignore_ws = self.diff_ignore_ws;
        let dark = Tokens::get(ctx).dark();
        spawn(&self.tx, ctx, move || {
            let result = diff::load(&path, &key, ignore_ws, dark);
            Msg::Diff(key, result)
        });
    }

    /// Starts comparing two points of history (replacing the comparison shown).
    fn compare(&mut self, compare: Compare, ctx: &egui::Context) {
        if matches!(
            self.diff,
            Some(DiffView {
                key: DiffKey::Range { .. },
                ..
            })
        ) {
            self.diff = None;
        }
        self.file_view = None;
        self.compare = Some(compare);
        self.reload_compare(ctx);
    }

    /// Reads the comparison's files again (the working tree or a branch moved).
    fn reload_compare(&mut self, ctx: &egui::Context) {
        self.compare_requests += 1;
        let request = self.compare_requests;
        let Some(c) = &mut self.compare else { return };
        c.request = request;
        let (path, base, target, since_fork) = (
            self.path.clone(),
            c.base.clone(),
            c.target.clone(),
            c.since_fork,
        );
        spawn(&self.tx, ctx, move || {
            let result = (|| {
                let base = git::resolve(&path, &base)?;
                let to = target.map(|t| git::resolve(&path, &t)).transpose()?;
                // The working tree goes with HEAD's history.
                let other = match &to {
                    Some(to) => to.clone(),
                    None => git::resolve(&path, "HEAD")?,
                };
                let from = if since_fork {
                    git::merge_base(&path, &base, &other)?
                } else {
                    base.clone()
                };
                Ok(CompareData {
                    files: git::diff_files(&path, &from, to.as_deref())?,
                    apart: git::count_apart(&path, &base, &other)?,
                    from,
                    to,
                })
            })();
            Msg::Compare(request, result)
        });
    }

    /// Shows `path`'s history or blame, loading what that tab needs.
    fn open_file_view(
        &mut self,
        path: String,
        rev: Option<String>,
        tab: file_view::FileTab,
        ctx: &egui::Context,
    ) {
        self.diff = None;
        self.conflict = None;
        let view = match self.file_view.take() {
            Some(mut v) if v.path == path && v.rev == rev => {
                v.tab = tab;
                v
            }
            _ => file_view::FileView::new(&path, rev.clone(), tab),
        };
        let needs = view.needs(tab);
        self.file_view = Some(view);
        if !needs {
            return;
        }
        let repo = self.path.clone();
        match tab {
            file_view::FileTab::History => spawn(&self.tx, ctx, move || {
                let result = git::file_history(&repo, rev.as_deref(), &path);
                Msg::FileHistory(path, result)
            }),
            file_view::FileTab::Blame => {
                let dark = Tokens::get(ctx).dark();
                spawn(&self.tx, ctx, move || {
                    let result = file_view::load_blame(&repo, rev.as_deref(), &path, dark);
                    Msg::Blame(path, rev, result)
                })
            }
        }
    }

    fn reopen_diff(&mut self, ctx: &egui::Context) {
        if let Some(view) = self.diff.take() {
            self.conflict = None;
            self.open_diff(view.key, ctx);
        }
    }

    /// Runs git commands one after another (stopping at the first failure), then refreshes.
    fn run_ops(
        &mut self,
        label: &str,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
        kind: OpKind,
        ctx: &egui::Context,
    ) {
        self.run_ops_ex(label, ops, refresh, kind, false, ctx);
    }

    fn run_ops_ex(
        &mut self,
        label: &str,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
        kind: OpKind,
        quiet: bool,
        ctx: &egui::Context,
    ) {
        if self.busy_op.is_some() {
            return;
        }
        self.busy_op = Some(label.to_owned());
        self.op_started = Some(Instant::now());
        if !quiet {
            self.status = None;
        }
        self.last_op = Some(PendingOp {
            label: label.to_owned(),
            ops: ops.clone(),
            refresh,
            kind: kind.clone(),
            quiet,
        });
        let path = self.path.clone();
        let label = label.to_owned();
        let creds = self.credentials.clone();
        spawn(&self.tx, ctx, move || {
            let mut result = Ok(String::new());
            for op in &ops {
                let args: Vec<&str> = op.iter().map(String::as_str).collect();
                result = git::run_with(Some(&path), &args, creds.as_ref());
                if result.is_err() {
                    break;
                }
            }
            // For HEAD moves, the new HEAD is what redo goes back to.
            let kind = match kind {
                OpKind::Normal(Undo::HeadMove { mode }, before) if result.is_ok() => {
                    OpKind::Normal(
                        Undo::HeadMove { mode },
                        format!("{before}\u{0}{}", git::head(&path)),
                    )
                }
                other => other,
            };
            Msg::OpDone {
                label,
                result,
                refresh,
                kind,
                quiet,
            }
        });
    }

    /// Runs `ops` as an undoable step.
    fn run_undoable(
        &mut self,
        label: &str,
        ops: Vec<Vec<String>>,
        refresh: Refresh,
        undo: Undo,
        ctx: &egui::Context,
    ) {
        let before = self.refs.head.clone();
        self.run_ops(label, ops, refresh, OpKind::Normal(undo, before), ctx);
    }

    fn checkout(&mut self, target: &str, ctx: &egui::Context) {
        let previous = self
            .refs
            .current
            .clone()
            .unwrap_or_else(|| self.refs.head.clone());
        let undo = if previous.is_empty() {
            Undo::None
        } else {
            Undo::Custom {
                undo: ops(&[&["checkout", &previous]]),
                redo: ops(&[&["checkout", target]]),
            }
        };
        self.run_undoable(
            &format!("Checkout {target}"),
            ops(&[&["checkout", target]]),
            Refresh::All,
            undo,
            ctx,
        );
    }

    /// Checkout and `pull --ff-only`; undoing goes back and puts the branch where it was.
    fn checkout_pull(&mut self, branch: &str, ctx: &egui::Context) {
        let previous = self
            .refs
            .current
            .clone()
            .unwrap_or_else(|| self.refs.head.clone());
        let Some(was) = self
            .refs
            .local
            .iter()
            .find(|b| b.name == branch)
            .map(|b| b.target.clone())
        else {
            return self.checkout(branch, ctx);
        };
        let run = ops(&[&["checkout", branch], &["pull", "--ff-only"]]);
        let undo = if previous.is_empty() {
            Undo::None
        } else {
            Undo::Custom {
                undo: ops(&[&["checkout", &previous], &["branch", "-f", branch, &was]]),
                redo: run.clone(),
            }
        };
        self.run_undoable(
            &format!("Checkout and pull {branch}"),
            run,
            Refresh::All,
            undo,
            ctx,
        );
    }

    /// The latest commit is amended (its message only, whatever is staged); an older one is
    /// reworded by an interactive rebase from its parent.
    fn reword_commit(&mut self, hash: String, message: String, ctx: &egui::Context) {
        if self.busy_op.is_some() {
            return;
        }
        self.reword = None;
        if hash == self.refs.head {
            let (subject, body) = message.split_once("\n\n").unwrap_or((&message, ""));
            let mut amend = vec![
                "commit",
                "--amend",
                "--only",
                "--allow-empty",
                "-m",
                subject,
            ];
            if !body.is_empty() {
                amend.extend(["-m", body]);
            }
            self.run_undoable(
                &format!("Edit the message of {}", short(&hash)),
                ops(&[&amend]),
                Refresh::All,
                Undo::HeadMove { mode: "--keep" },
                ctx,
            );
            return;
        }
        let path = self.path.clone();
        spawn(&self.tx, ctx, move || {
            let result = git::rebase_commits(&path, &format!("{hash}^"));
            Msg::RewordCommits {
                hash,
                message,
                result,
            }
        });
    }

    fn checkout_remote(&mut self, remote_branch: &str, ctx: &egui::Context) {
        // `origin/feature` → local `feature` tracking it (or just switch if it exists).
        let local = remote_branch
            .split_once('/')
            .map(|(_, b)| b)
            .unwrap_or(remote_branch)
            .to_owned();
        if self.refs.local.iter().any(|b| b.name == local) {
            self.checkout(&local, ctx);
            return;
        }
        let previous = self
            .refs
            .current
            .clone()
            .unwrap_or_else(|| self.refs.head.clone());
        self.run_undoable(
            &format!("Checkout {local}"),
            ops(&[&["checkout", "--track", remote_branch]]),
            Refresh::All,
            Undo::Custom {
                undo: ops(&[&["checkout", &previous], &["branch", "-D", &local]]),
                redo: ops(&[&["checkout", "--track", remote_branch]]),
            },
            ctx,
        );
    }

    /// Once per tab, on a GitHub / GitLab repository: tells the avatar loader where its authors'
    /// account pictures are.
    fn hint_avatars(&mut self, ctx: &egui::Context) {
        if self.avatars_hinted || !Settings::get(ctx).avatars {
            return;
        }
        let Some(forge) = self.forge else { return };
        let Some((host, path)) = self
            .remotes
            .iter()
            .find(|r| r.name == "origin")
            .or_else(|| self.remotes.first())
            .and_then(|r| forge::location(&r.url))
        else {
            return;
        };
        self.avatars_hinted = true;
        let repo = self.path.clone();
        let mut emails: Vec<String> = self.history.iter().map(|c| c.email.clone()).collect();
        emails.sort();
        emails.dedup();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            match forge {
                Forge::GitHub => {
                    for (email, url) in forge::github_avatars(&repo, &host, &path) {
                        crate::avatars::hint(&email, crate::avatars::Hint::Url(url));
                    }
                }
                Forge::GitLab => {
                    for email in emails {
                        crate::avatars::hint(&email, crate::avatars::Hint::GitLab(host.clone()));
                    }
                }
            }
            ctx.request_repaint();
        });
    }

    /// HTTPS credentials for this tab's git commands (from the clone that made it).
    pub fn set_credentials(&mut self, creds: git::Credentials) {
        self.credentials = Some(creds);
    }

    fn default_remote(&self) -> Option<String> {
        self.remotes
            .iter()
            .map(|r| r.name.as_str())
            .find(|r| *r == "origin")
            .or_else(|| self.remotes.first().map(|r| r.name.as_str()))
            .map(str::to_owned)
    }

    fn push(&mut self, ctx: &egui::Context) {
        let Some(current) = self.refs.current.clone() else {
            self.status = Some(("Push needs a checked-out branch".to_owned(), true));
            return;
        };
        let has_upstream = self
            .refs
            .local
            .iter()
            .any(|b| b.name == current && !b.upstream.is_empty() && !b.upstream_gone());
        if has_upstream {
            self.run_ops(
                "Push",
                ops(&[&["push"]]),
                Refresh::All,
                OpKind::Normal(Undo::None, String::new()),
                ctx,
            );
            return;
        }
        // First push of a branch: publish it and track the remote one.
        match self.default_remote() {
            Some(remote) => self.run_ops(
                &format!("Push {current} to {remote}"),
                ops(&[&["push", "-u", &remote, &current]]),
                Refresh::All,
                OpKind::Normal(Undo::None, String::new()),
                ctx,
            ),
            None => {
                self.status = Some(("This repository has no remote to push to".to_owned(), true))
            }
        }
    }

    fn commit(&mut self, ctx: &egui::Context) {
        let summary = self.commit_summary.trim();
        if summary.is_empty() {
            self.status = Some(("Write a commit summary first".to_owned(), true));
            return;
        }
        if !self.work.conflicted.is_empty() {
            self.status = Some(("Resolve the conflicted files first".to_owned(), true));
            return;
        }
        let mut commit = vec!["commit".to_owned()];
        if self.amend {
            commit.push("--amend".to_owned());
        }
        commit.extend(["-m".to_owned(), summary.to_owned()]);
        let body = self.commit_body.trim();
        if !body.is_empty() {
            commit.extend(["-m".to_owned(), body.to_owned()]);
        }
        let mut steps = Vec::new();
        // A rebase stopped on conflicts finishes with `rebase --continue`, not a new commit.
        if self.work.in_progress == Some(InProgress::Rebase) {
            steps.push(vec!["rebase".to_owned(), "--continue".to_owned()]);
            self.run_ops(
                "Continue rebase",
                steps,
                Refresh::All,
                OpKind::Normal(Undo::None, String::new()),
                ctx,
            );
            return;
        }
        // Nothing staged: commit everything, like "Stage all and commit" (an amend with
        // nothing staged only rewrites the message).
        if self.work.staged.is_empty() && !self.amend {
            steps.push(vec!["add".to_owned(), "-A".to_owned()]);
        }
        steps.push(commit);
        let label = if self.amend { "Amend" } else { "Commit" };
        self.run_undoable(
            label,
            steps,
            Refresh::All,
            Undo::HeadMove { mode: "--soft" },
            ctx,
        );
    }

    fn stash_push(&mut self, message: &str, ctx: &egui::Context) {
        let mut args = vec!["stash", "push", "--include-untracked"];
        if !message.trim().is_empty() {
            args.extend(["-m", message.trim()]);
        }
        let redo = ops(&[&args]);
        self.run_undoable(
            "Stash",
            ops(&[&args]),
            Refresh::All,
            Undo::Custom {
                undo: ops(&[&["stash", "pop", "--index"]]),
                redo,
            },
            ctx,
        );
    }

    fn undo(&mut self, ctx: &egui::Context) {
        if self.busy_op.is_some() {
            return;
        }
        if let Some(entry) = self.undo_stack.pop() {
            let label = format!("Undo {}", entry.label);
            let steps = entry.undo.clone();
            self.run_ops(&label, steps, Refresh::All, OpKind::Undo(entry), ctx);
        }
    }

    fn redo(&mut self, ctx: &egui::Context) {
        if self.busy_op.is_some() {
            return;
        }
        if let Some(entry) = self.redo_stack.pop() {
            let label = format!("Redo {}", entry.label);
            let steps = entry.redo.clone();
            self.run_ops(&label, steps, Refresh::All, OpKind::Redo(entry), ctx);
        }
    }

    fn run_search(&mut self, ctx: &egui::Context) {
        let query = self.search.query.trim().to_owned();
        self.search.edited = None;
        if query.is_empty() {
            self.search.results.clear();
            self.search.matches.clear();
            self.search.searched.clear();
            return;
        }
        self.search.running = true;
        let path = self.path.clone();
        let code = self.search.code;
        spawn(&self.tx, ctx, move || {
            let result = if code {
                git::search_code(&path, &query)
            } else {
                git::search(&path, &query)
            };
            Msg::Search(query, result)
        });
    }

    fn search_step(&mut self, delta: i32, ctx: &egui::Context) {
        let n = self.search.results.len();
        if n == 0 {
            return;
        }
        let cur = self.search.current as i32 + delta;
        self.search.current = cur.rem_euclid(n as i32) as usize;
        if let Some(hash) = self.search.results.get(self.search.current).cloned() {
            self.goto(&hash, ctx);
        }
    }

    fn start_terminal(
        &mut self,
        shell: Option<ShellSpec>,
        cwd: Option<PathBuf>,
        ctx: &egui::Context,
    ) {
        let shell = shell.unwrap_or_else(|| crate::terminal::default_shell(ctx));
        let cwd = cwd.unwrap_or_else(|| self.path.clone());
        match Terminal::spawn(&cwd, &shell, ctx) {
            Ok(term) => {
                self.terminals.push(term);
                self.active_terminal = self.terminals.len() - 1;
                self.terminal_error = None;
                self.terminal_open = true;
            }
            Err(e) => {
                self.terminal_error = Some(e);
                self.terminal_open = true;
            }
        }
    }

    /// Drains finished background work and file-system events.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg, ctx);
        }

        // Changes on disk, debounced.
        if let Some(rx) = &self.fs_rx {
            while let Ok(git_dir) = rx.try_recv() {
                let was = self.fs_pending.map(|(g, _)| g).unwrap_or(false);
                self.fs_pending = Some((was || git_dir, Instant::now()));
            }
        }
        if let Some((git_dir, at)) = self.fs_pending
            && at.elapsed() > Duration::from_millis(400)
            && !self.loading
            && self.busy_op.is_none()
        {
            self.fs_pending = None;
            if git_dir {
                self.reload(ctx);
            } else {
                self.reload_status(ctx);
            }
        }

        // Background fetch.
        let minutes = Settings::get(ctx).auto_fetch_minutes;
        if minutes > 0
            && !self.remotes.is_empty()
            && self.busy_op.is_none()
            && self.last_fetch.elapsed() > Duration::from_secs(u64::from(minutes) * 60)
        {
            self.last_fetch = Instant::now();
            self.run_ops_ex(
                "Background fetch",
                ops(&[&["fetch", "--all", "--prune", "--quiet"]]),
                Refresh::All,
                OpKind::Normal(Undo::None, String::new()),
                true,
                ctx,
            );
        }
        if minutes > 0 {
            ctx.request_repaint_after(Duration::from_secs(30));
        }

        // Debounced search while typing.
        if let Some(at) = self.search.edited
            && at.elapsed() > Duration::from_millis(350)
        {
            self.run_search(ctx);
        } else if self.search.edited.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn handle(&mut self, msg: Msg, ctx: &egui::Context) {
        match msg {
            Msg::Loaded(result) => {
                self.loading = false;
                match result {
                    Ok(loaded) => {
                        self.load_error = None;
                        self.refs = loaded.refs;
                        self.has_more = loaded.history.len() >= loaded.requested;
                        self.history = loaded.history;
                        self.work = loaded.work;
                        self.worktrees = loaded.worktrees;
                        self.stashes = loaded.stashes;
                        if self.stash_view.is_some_and(|i| i >= self.stashes.len()) {
                            self.stash_view = None;
                        }
                        self.identity = loaded.identity;
                        let forge = forge::detect(&loaded.remotes);
                        self.remotes = loaded.remotes;
                        if forge != self.forge {
                            self.forge = forge;
                            self.prs.clear();
                            self.prs_loaded = None;
                        }
                        if self
                            .prs_loaded
                            .is_none_or(|t| t.elapsed() > Duration::from_secs(120))
                        {
                            self.load_prs(ctx);
                        }
                        self.rebuild(ctx);
                        self.hint_avatars(ctx);
                        self.reload_compare(ctx);
                    }
                    Err(e) => self.load_error = Some(e),
                }
            }
            Msg::More(result) => {
                self.loading_more = false;
                match result {
                    Ok(page) => {
                        self.has_more = page.len() == git::LOG_PAGE;
                        // A reload in between may have loaded these already.
                        let known: HashSet<&str> =
                            self.history.iter().map(|c| c.hash.as_str()).collect();
                        let fresh: Vec<Commit> = page
                            .into_iter()
                            .filter(|c| !known.contains(c.hash.as_str()))
                            .collect();
                        self.history.extend(fresh);
                        self.rebuild(ctx);
                    }
                    Err(e) => self.status = Some((e, true)),
                }
            }
            Msg::Status(result) => match result {
                Ok(work) => {
                    self.work = work;
                    self.rebuild(ctx);
                    if self.compare.as_ref().is_some_and(|c| c.target.is_none()) {
                        self.reload_compare(ctx);
                    }
                }
                Err(e) => self.status = Some((e, true)),
            },
            Msg::Detail(hash, result) => {
                if self.detail_hash.as_deref() == Some(hash.as_str()) {
                    match result {
                        Ok(d) => self.detail = Some(d),
                        Err(e) => self.detail_error = Some(e),
                    }
                }
            }
            Msg::RebaseCommits {
                onto,
                label,
                result,
            } => match result {
                Ok(commits) if commits.is_empty() => {
                    self.status = Some((
                        format!(
                            "Nothing to rebase: {label} already contains every commit of this branch"
                        ),
                        true,
                    ));
                }
                Ok(commits) => {
                    self.diff = None;
                    self.conflict = None;
                    self.file_view = None;
                    self.rebase = Some(rebase::RebaseEditor::new(onto, label, commits));
                }
                Err(e) => self.status = Some((format!("Interactive rebase: {e}"), true)),
            },
            Msg::RewordCommits {
                hash,
                message,
                result,
            } => {
                let result = result.and_then(|commits| {
                    let onto = format!("{hash}^");
                    let mut editor =
                        rebase::RebaseEditor::new(onto, short(&hash).to_owned(), commits);
                    let entry = editor
                        .entries
                        .iter_mut()
                        .find(|e| e.commit.hash == hash)
                        .ok_or("only commits of the checked-out branch can be edited")?;
                    entry.step = rebase::Step::Reword;
                    entry.message = message;
                    if let Some(problem) = editor.problem() {
                        return Err(problem);
                    }
                    editor.command()
                });
                match result {
                    Ok(command) => self.run_undoable(
                        &format!("Edit the message of {}", short(&hash)),
                        vec![command],
                        Refresh::All,
                        Undo::HeadMove { mode: "--keep" },
                        ctx,
                    ),
                    Err(e) => self.status = Some((format!("Edit message: {e}"), true)),
                }
            }
            Msg::FileHistory(path, result) => {
                if let Some(view) = &mut self.file_view
                    && view.path == path
                {
                    view.history = Some(result);
                }
            }
            Msg::Blame(path, rev, result) => {
                if let Some(view) = &mut self.file_view
                    && view.path == path
                    && view.rev == rev
                {
                    view.blame = Some(result);
                }
            }
            Msg::Diff(key, result) => {
                if let Some(view) = &mut self.diff
                    && view.key == key
                {
                    view.data = Some(result);
                    // The lines may have moved.
                    view.chosen.clear();
                    view.anchor = None;
                }
            }
            Msg::Compare(request, result) => {
                if let Some(c) = &mut self.compare
                    && c.request == request
                {
                    c.data = Some(result);
                }
            }
            Msg::Search(query, result) => {
                if query == self.search.query.trim() {
                    self.search.running = false;
                    self.search.searched = query;
                    match result {
                        Ok(hashes) => {
                            self.search.matches = hashes.iter().cloned().collect();
                            self.search.results = hashes;
                            self.search.current = 0;
                            if let Some(first) = self.search.results.first().cloned() {
                                self.goto(&first, ctx);
                            }
                        }
                        Err(e) => self.status = Some((e, true)),
                    }
                }
            }
            Msg::Prs(result) => match result {
                Ok(prs) => {
                    self.prs = prs;
                    self.pr_error = None;
                }
                Err(e) => self.pr_error = Some(e),
            },
            Msg::OpDone {
                label,
                result,
                refresh,
                kind,
                quiet,
            } => {
                self.busy_op = None;
                let long = self
                    .op_started
                    .take()
                    .is_some_and(|t| t.elapsed() > LONG_OP);
                match &result {
                    Ok(_) => {
                        if label == "Commit" || label == "Amend" || label == "Continue rebase" {
                            self.commit_summary.clear();
                            self.commit_body.clear();
                            self.amend = false;
                        }
                        // Quiet success for staging, which happens all the time.
                        if refresh == Refresh::All && !quiet {
                            self.status = Some((format!("{label}: done"), false));
                        }
                        if label.contains("etch") {
                            self.load_prs(ctx);
                        }
                        self.record_undo(&label, kind);
                    }
                    Err(e) if git::is_auth_error(e) && !quiet => {
                        // Typed credentials that were refused are forgotten.
                        let refused = self.credentials.take().is_some();
                        self.status = Some((format!("{label}: sign-in needed"), true));
                        let url = self
                            .default_remote()
                            .and_then(|r| self.remotes.iter().find(|x| x.name == r))
                            .map(|r| r.url.clone())
                            .unwrap_or_default();
                        self.dialog = Some(Dialog::credentials(&url, refused));
                    }
                    Err(e) => {
                        self.focus_wip = e.contains("CONFLICT") || e.contains("could not apply");
                        self.status = Some((format!("{label} failed: {e}"), true));
                        // A failed undo stays undoable.
                        match kind {
                            OpKind::Undo(entry) => self.undo_stack.push(entry),
                            OpKind::Redo(entry) => self.redo_stack.push(entry),
                            OpKind::Normal(..) => {}
                        }
                    }
                }
                if long && !quiet {
                    self.finished = Some(match &result {
                        Ok(_) => (format!("{} · {label}: done", self.name), false),
                        Err(e) => (
                            format!("{} · {label} failed: {}", self.name, headline(e)),
                            true,
                        ),
                    });
                }
                match refresh {
                    Refresh::Status => self.reload_status(ctx),
                    Refresh::All => self.reload(ctx),
                }
            }
        }
    }

    fn record_undo(&mut self, label: &str, kind: OpKind) {
        match kind {
            OpKind::Normal(Undo::None, _) => {}
            OpKind::Normal(Undo::HeadMove { mode }, heads) => {
                let (before, after) = heads.split_once('\u{0}').unwrap_or(("", ""));
                if before.is_empty() || after.is_empty() || before == after {
                    return;
                }
                self.undo_stack.push(UndoEntry {
                    label: label.to_owned(),
                    undo: ops(&[&["reset", mode, before]]),
                    redo: ops(&[&["reset", mode, after]]),
                });
                self.redo_stack.clear();
            }
            OpKind::Normal(Undo::Custom { undo, redo }, _) => {
                self.undo_stack.push(UndoEntry {
                    label: label.to_owned(),
                    undo,
                    redo,
                });
                self.redo_stack.clear();
            }
            OpKind::Undo(entry) => self.redo_stack.push(entry),
            OpKind::Redo(entry) => self.undo_stack.push(entry),
        }
        // Plenty for a session.
        if self.undo_stack.len() > 50 {
            self.undo_stack.remove(0);
        }
    }

    pub(crate) fn apply(&mut self, action: Action, ctx: &egui::Context) {
        let plain = OpKind::Normal(Undo::None, String::new());
        match action {
            Action::Select(row) => self.select(row, ctx),
            Action::SelectHash(hash) => self.goto(&hash, ctx),
            Action::SelectStash(i) => self.select_stash(i, ctx),
            Action::OpenDiff(key) => self.open_diff(key, ctx),
            Action::CloseDiff => {
                self.diff = None;
                self.conflict = None;
            }
            Action::Ops {
                label,
                ops,
                refresh,
            } => self.run_ops(&label, ops, refresh, plain, ctx),
            Action::UndoableOps {
                label,
                ops,
                refresh,
                undo,
            } => self.run_undoable(&label, ops, refresh, undo, ctx),
            Action::Checkout(name) => self.checkout(&name, ctx),
            Action::CheckoutPull(name) => self.checkout_pull(&name, ctx),
            Action::CheckoutRemote(name) => self.checkout_remote(&name, ctx),
            Action::Dialog(dialog) => self.dialog = Some(dialog),
            Action::Commit => self.commit(ctx),
            Action::Fetch => {
                self.last_fetch = Instant::now();
                self.run_ops(
                    "Fetch",
                    ops(&[&["fetch", "--all", "--prune"]]),
                    Refresh::All,
                    plain,
                    ctx,
                )
            }
            Action::Pull => self.run_undoable(
                "Pull",
                ops(&[&["pull", "--ff-only"]]),
                Refresh::All,
                Undo::HeadMove { mode: "--keep" },
                ctx,
            ),
            Action::Push => self.push(ctx),
            Action::Reload => self.reload(ctx),
            Action::LoadMore => self.load_more(ctx),
            Action::Undo => self.undo(ctx),
            Action::Redo => self.redo(ctx),
            Action::StashPush(message) => self.stash_push(&message, ctx),
            Action::OpenTab(path) => self.open_request = Some(path),
            Action::ToggleTerminal => {
                self.terminal_open = !self.terminal_open;
                if self.terminal_open && self.terminals.is_empty() {
                    self.start_terminal(None, None, ctx);
                }
            }
            Action::NewTerminal { shell, cwd } => self.start_terminal(shell, cwd, ctx),
            Action::CloseTerminal(i) => {
                if i < self.terminals.len() {
                    self.terminals.remove(i);
                }
                self.active_terminal = self
                    .active_terminal
                    .min(self.terminals.len().saturating_sub(1));
                if self.terminals.is_empty() {
                    self.terminal_open = false;
                }
            }
            Action::SelectTerminal(i) => self.active_terminal = i,
            Action::ToggleSearch => {
                self.search.open = !self.search.open;
                self.search.focus = self.search.open;
                if !self.search.open {
                    self.search.matches.clear();
                    self.search.results.clear();
                    self.search.searched.clear();
                }
            }
            Action::SearchStep(d) => self.search_step(d, ctx),
            Action::CreatePr {
                branch,
                base,
                title,
                body,
                draft,
            } => {
                let Some(forge) = self.forge else { return };
                if self.busy_op.is_some() {
                    return;
                }
                let label = format!("Create {}", forge.noun());
                self.busy_op = Some(label.clone());
                self.op_started = Some(Instant::now());
                let path = self.path.clone();
                spawn(&self.tx, ctx, move || {
                    // Publish the branch first, so the forge can see it.
                    let pushed = git::run(Some(&path), &["push", "-u", "origin", &branch]);
                    let result = pushed.and_then(|_| {
                        forge::create(&path, forge, &branch, &base, &title, &body, draft)
                    });
                    Msg::OpDone {
                        label,
                        result,
                        refresh: Refresh::All,
                        kind: OpKind::Normal(Undo::None, String::new()),
                        quiet: false,
                    }
                });
            }
            Action::OpenUrl(url) => {
                let _ = open::that_detached(url);
            }
            Action::OpenPath(rel) => {
                let _ = open::that_detached(self.path.join(rel));
            }
            Action::OpenInEditor(rel) => {
                let editor = Settings::get(ctx).editor;
                if let Err(e) = open_in_editor(&editor, &self.path.join(rel)) {
                    self.status = Some((e, true));
                }
            }
            Action::SetIdentity(name, email) => self.run_ops(
                "Set identity",
                ops(&[
                    &["config", "--local", "user.name", &name],
                    &["config", "--local", "user.email", &email],
                ]),
                Refresh::All,
                plain,
                ctx,
            ),
            Action::SetDiffOptions { split, ignore_ws } => {
                self.diff_split = split;
                if ignore_ws != self.diff_ignore_ws {
                    self.diff_ignore_ws = ignore_ws;
                    self.reopen_diff(ctx);
                }
            }
            Action::FileView { path, rev, tab } => self.open_file_view(path, rev, tab, ctx),
            Action::CloseFileView => self.file_view = None,
            Action::InteractiveRebase { onto, label } => {
                let path = self.path.clone();
                spawn(&self.tx, ctx, move || {
                    let result = git::rebase_commits(&path, &onto);
                    Msg::RebaseCommits {
                        onto,
                        label,
                        result,
                    }
                });
            }
            Action::Reword { hash, message } => self.reword_commit(hash, message, ctx),
            Action::RunRebase => {
                let Some(editor) = &self.rebase else { return };
                if editor.problem().is_some() || self.busy_op.is_some() {
                    return;
                }
                match editor.command() {
                    Ok(command) => {
                        let label = format!("Interactive rebase onto {}", editor.onto_label);
                        self.rebase = None;
                        self.run_undoable(
                            &label,
                            vec![command],
                            Refresh::All,
                            Undo::HeadMove { mode: "--keep" },
                            ctx,
                        );
                    }
                    Err(e) => self.status = Some((e, true)),
                }
            }
            Action::CloseRebase => self.rebase = None,
            Action::SignIn(creds) => {
                self.credentials = Some(creds);
                if let Some(op) = self.last_op.take() {
                    self.run_ops_ex(&op.label, op.ops, op.refresh, op.kind, op.quiet, ctx);
                }
            }
            Action::Ignore { pattern, path } => match git::add_to_gitignore(&self.path, &pattern) {
                Err(e) => self.status = Some((e, true)),
                Ok(added) => {
                    if git::is_tracked(&self.path, &path) {
                        self.run_ops(
                            &format!("Ignore {pattern}"),
                            ops(&[&["rm", "-r", "--cached", "--quiet", "--", &path]]),
                            Refresh::Status,
                            plain,
                            ctx,
                        );
                    } else {
                        let msg = if added {
                            format!("Added {pattern} to .gitignore")
                        } else {
                            format!("{pattern} was already in .gitignore")
                        };
                        self.status = Some((msg, false));
                        self.reload_status(ctx);
                    }
                }
            },
            Action::PickLines { pick, lines } => {
                let Some(DiffView {
                    key: DiffKey::Work(file),
                    data: Some(Ok(data)),
                    ..
                }) = &self.diff
                else {
                    return;
                };
                let Some(patch) = diff::partial_patch(&data.header, &data.lines, &lines, pick)
                else {
                    return;
                };
                let patch = match git::save_patch(&patch) {
                    Ok(p) => p.to_string_lossy().into_owned(),
                    Err(e) => {
                        self.status = Some((e, true));
                        return;
                    }
                };
                let n = lines.len();
                let label = format!(
                    "{} {n} line{} of {}",
                    pick.verb(),
                    if n == 1 { "" } else { "s" },
                    file.path
                );
                let mut args = pick.args().to_vec();
                args.push(&patch);
                if pick == diff::Pick::Discard {
                    // Applying the same patch forward brings the lines back.
                    let undo = Undo::Custom {
                        undo: ops(&[&["apply", &patch]]),
                        redo: ops(&[&args]),
                    };
                    self.run_undoable(&label, ops(&[&args]), Refresh::Status, undo, ctx);
                } else {
                    self.run_ops(&label, ops(&[&args]), Refresh::Status, plain, ctx);
                }
            }
            Action::Compare {
                base,
                base_label,
                target,
                target_label,
                since_fork,
            } => self.compare(
                Compare {
                    base,
                    base_label,
                    target,
                    target_label,
                    since_fork,
                    data: None,
                    request: 0,
                },
                ctx,
            ),
            Action::CompareSinceFork(on) => {
                if let Some(c) = &mut self.compare {
                    c.since_fork = on;
                    self.reload_compare(ctx);
                }
            }
            Action::SwapCompare => {
                // The working tree stays the newer end.
                if let Some(c) = self.compare.take_if(|c| c.target.is_some())
                    && let Some(target) = c.target
                {
                    self.compare(
                        Compare {
                            base: target,
                            base_label: c.target_label,
                            target: Some(c.base),
                            target_label: c.base_label,
                            data: None,
                            ..c
                        },
                        ctx,
                    );
                }
            }
            Action::CloseCompare => {
                self.compare = None;
                if matches!(
                    self.diff,
                    Some(DiffView {
                        key: DiffKey::Range { .. },
                        ..
                    })
                ) {
                    self.diff = None;
                }
            }
            Action::SearchCode(on) => {
                if self.search.code != on {
                    self.search.code = on;
                    self.run_search(ctx);
                }
                self.search.focus = true;
            }
            Action::SaveConflict => {
                if let Some(editor) = &self.conflict {
                    match editor.save(&self.path) {
                        Ok(()) => {
                            let file = editor.path.clone();
                            self.run_ops(
                                "Mark resolved",
                                ops(&[&["add", "--", &file]]),
                                Refresh::Status,
                                plain,
                                ctx,
                            );
                        }
                        Err(e) => self.status = Some((e, true)),
                    }
                }
            }
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        let t = Tokens::get(&ctx);
        let mut acts: Vec<Action> = std::mem::take(&mut self.queued);

        let free_keys =
            self.dialog.is_none() && !self.keys_blocked && !ctx.egui_wants_keyboard_input();
        if free_keys {
            let (esc, undo, redo, find) = ctx.input(|i| {
                (
                    i.key_pressed(Key::Escape),
                    i.modifiers.command && !i.modifiers.shift && i.key_pressed(Key::Z),
                    i.modifiers.command
                        && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
                    i.modifiers.command && i.key_pressed(Key::F),
                )
            });
            if esc && self.diff.is_some() {
                acts.push(Action::CloseDiff);
            } else if esc && self.rebase.is_some() {
                acts.push(Action::CloseRebase);
            } else if esc && self.file_view.is_some() {
                acts.push(Action::CloseFileView);
            } else if esc && self.compare.is_some() {
                acts.push(Action::CloseCompare);
            } else if esc && self.search.open {
                acts.push(Action::ToggleSearch);
            }
            if undo {
                acts.push(Action::Undo);
            }
            if redo {
                acts.push(Action::Redo);
            }
            if find {
                self.search.open = true;
                self.search.focus = true;
            }
        } else if self.search.open && ctx.input(|i| i.modifiers.command && i.key_pressed(Key::F)) {
            self.search.focus = true;
        }

        egui::Panel::top("repo_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(t.chrome)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .stroke(egui::Stroke::new(1.0, t.separator)),
            )
            .show(ui, |ui| self.toolbar(ui, &mut acts));
        let side = egui::Frame::new().fill(t.card);
        egui::Panel::left("repo_branches")
            .resizable(true)
            .default_size(270.0)
            .size_range(170.0..=520.0)
            .frame(side)
            .show(ui, |ui| self.sidebar(ui, &mut acts));
        egui::Panel::right("repo_detail")
            .resizable(true)
            .default_size(420.0)
            .size_range(280.0..=900.0)
            .frame(side)
            .show(ui, |ui| self.detail_panel(ui, &mut acts));
        if self.terminal_open {
            egui::Panel::bottom("repo_terminal")
                .resizable(true)
                .default_size(260.0)
                .size_range(90.0..=900.0)
                .frame(egui::Frame::new().fill(t.card))
                .show(ui, |ui| self.terminal_panel(ui, &mut acts));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.dock))
            .show(ui, |ui| {
                if self.conflict.is_some() {
                    self.conflict_view(ui, &mut acts);
                } else if self.rebase.is_some() {
                    self.rebase_view(ui, &mut acts);
                } else if self.diff.is_some() {
                    self.diff_view(ui, &mut acts);
                } else if self.file_view.is_some() {
                    self.file_view(ui, &mut acts);
                } else {
                    self.graph_view(ui, &mut acts);
                }
            });

        self.dialogs(&ctx, &mut acts);
        for action in acts {
            self.apply(action, &ctx);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let idle = self.busy_op.is_none();

            let undo_tip = match self.undo_stack.last() {
                Some(e) => format!("Undo {} (Ctrl+Z)", e.label),
                None => "Nothing to undo".to_owned(),
            };
            if icons::tool_button(ui, "undo-2", "", idle && !self.undo_stack.is_empty())
                .on_hover_text(undo_tip)
                .on_disabled_hover_text("Nothing to undo")
                .clicked()
            {
                acts.push(Action::Undo);
            }
            let redo_tip = match self.redo_stack.last() {
                Some(e) => format!("Redo {} (Ctrl+Y)", e.label),
                None => "Nothing to redo".to_owned(),
            };
            if icons::tool_button(ui, "redo-2", "", idle && !self.redo_stack.is_empty())
                .on_hover_text(redo_tip)
                .on_disabled_hover_text("Nothing to redo")
                .clicked()
            {
                acts.push(Action::Redo);
            }
            widgets::toolbar_separator(ui);

            let has_remote = !self.remotes.is_empty();
            let fetched = format!(
                "Fetch all remotes (git fetch --all --prune)\nLast fetched {} ago",
                short_duration(self.last_fetch.elapsed())
            );
            if icons::tool_button(ui, "cloud-download", "Fetch", idle && has_remote)
                .on_hover_text(fetched)
                .clicked()
            {
                acts.push(Action::Fetch);
            }
            let (ahead, behind) = self
                .refs
                .current
                .as_ref()
                .and_then(|c| self.refs.local.iter().find(|b| &b.name == c))
                .map(|b| b.ahead_behind())
                .unwrap_or((0, 0));
            if icons::tool_button_count(
                ui,
                "arrow-down-to-line",
                "Pull",
                behind,
                idle && has_remote,
            )
            .on_hover_text("Fast-forward the current branch (git pull --ff-only)")
            .clicked()
            {
                acts.push(Action::Pull);
            }
            if icons::tool_button_count(ui, "arrow-up-from-line", "Push", ahead, idle && has_remote)
                .on_hover_text("Push the current branch (publishes it if it has no upstream yet)")
                .clicked()
            {
                acts.push(Action::Push);
            }
            widgets::toolbar_separator(ui);
            let head = self.refs.head.clone();
            if icons::tool_button(ui, "git-branch-plus", "Branch", idle && !head.is_empty())
                .on_hover_text("Create a branch at HEAD")
                .clicked()
            {
                acts.push(Action::Dialog(Dialog::create_branch(&head, "HEAD")));
            }
            if icons::tool_button(ui, "archive", "Stash", idle && !self.work.is_clean())
                .on_hover_text("Stash all changes, untracked files included")
                .on_disabled_hover_text("No changes to stash")
                .clicked()
            {
                acts.push(Action::Dialog(Dialog::stash()));
            }
            let terminal = icons::tool_button(ui, "square-terminal", "Terminal", true)
                .on_hover_text("Terminal in the repository folder");
            if self.terminal_open {
                widgets::active_underline(ui, terminal.rect, t.accent);
            }
            if terminal.clicked() {
                acts.push(Action::ToggleTerminal);
            }
            let search =
                icons::tool_button(ui, "search", "", true).on_hover_text("Search commits (Ctrl+F)");
            if self.search.open {
                widgets::active_underline(ui, search.rect, t.accent);
            }
            if search.clicked() {
                acts.push(Action::ToggleSearch);
            }
            let open =
                icons::tool_button(ui, "external-link", "", true).on_hover_text("Open the folder…");
            egui::Popup::menu(&open)
                .style(crate::theme::menu_style)
                .show(|ui| {
                    if ui.button(reveal_label()).clicked() {
                        acts.push(Action::OpenPath(PathBuf::new()));
                    }
                    if ui.button("Open in editor").clicked() {
                        acts.push(Action::OpenInEditor(PathBuf::new()));
                    }
                    if ui.button("Copy path").clicked() {
                        ui.ctx().copy_text(self.path.display().to_string());
                    }
                });
            widgets::toolbar_separator(ui);
            if icons::tool_button(ui, "refresh-cw", "", !self.loading)
                .on_hover_text("Reload (F5)")
                .clicked()
            {
                acts.push(Action::Reload);
            }
            widgets::toolbar_separator(ui);

            match &self.refs.current {
                Some(branch) => {
                    icons::inline(ui, "git-branch", 14.0, t.text_dim);
                    ui.label(egui::RichText::new(branch).font(crate::theme::medium(12.5)));
                }
                None if self.refs.head.is_empty() => {
                    ui.label(egui::RichText::new("No commits yet").color(t.text_dim));
                }
                None => {
                    ui.label(
                        egui::RichText::new(format!(
                            "Detached HEAD at {}",
                            self.refs.head.get(..8).unwrap_or(&self.refs.head)
                        ))
                        .color(t.warning),
                    );
                }
            }

            if let Some(state) = self.work.in_progress {
                ui.add_space(10.0);
                widgets::chip(ui, &format!("{} in progress", state.label()), t.warning);
                if ui
                    .add_enabled(
                        idle,
                        egui::Button::new(format!("Abort {}", state.label().to_lowercase())),
                    )
                    .clicked()
                {
                    acts.push(abort_action(state));
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((msg, is_err)) = &self.status {
                    let color = if *is_err { t.danger } else { t.text_dim };
                    let first_line = headline(msg);
                    ui.add(
                        egui::Label::new(egui::RichText::new(first_line).color(color)).truncate(),
                    )
                    .on_hover_text(msg.as_str());
                }
                if let Some(op) = &self.busy_op {
                    ui.label(egui::RichText::new(format!("{op}…")).color(t.text_dim));
                    ui.spinner();
                } else if self.loading {
                    ui.spinner();
                }
            });
        });
    }

    /// The bottom panel: terminal tabs and the active shell.
    fn terminal_panel(&mut self, ui: &mut Ui, acts: &mut Vec<Action>) {
        let t = Tokens::get(ui.ctx());
        widgets::terminal_tabs(ui, &self.terminals, self.active_terminal, acts);
        if let Some(err) = &self.terminal_error {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.colored_label(t.danger, err);
            });
            if self.terminals.is_empty() {
                return;
            }
        }
        let i = self.active_terminal;
        let Some(term) = self.terminals.get_mut(i) else {
            return;
        };
        if term.has_exited() {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new("The shell has exited.").color(t.text_dim));
                if ui.button("Close").clicked() {
                    acts.push(Action::CloseTerminal(i));
                }
            });
            return;
        }
        term.ui(ui);
    }

    /// What the command palette offers for this repository.
    pub(crate) fn palette_items(&self) -> Vec<(String, String, Action)> {
        let mut items: Vec<(String, String, Action)> = vec![
            ("Fetch".into(), "remote".into(), Action::Fetch),
            ("Pull".into(), "remote".into(), Action::Pull),
            ("Push".into(), "remote".into(), Action::Push),
            ("Undo".into(), "Ctrl+Z".into(), Action::Undo),
            ("Redo".into(), "Ctrl+Y".into(), Action::Redo),
            (
                "Stash changes".into(),
                "stash".into(),
                Action::Dialog(Dialog::stash()),
            ),
            (
                "Search commits".into(),
                "Ctrl+F".into(),
                Action::ToggleSearch,
            ),
            (
                "Toggle terminal".into(),
                "terminal".into(),
                Action::ToggleTerminal,
            ),
            (
                "New terminal".into(),
                "terminal".into(),
                Action::NewTerminal {
                    shell: None,
                    cwd: None,
                },
            ),
            ("Reload".into(), "F5".into(), Action::Reload),
            (
                reveal_label().into(),
                "folder".into(),
                Action::OpenPath(PathBuf::new()),
            ),
            (
                "Open in editor".into(),
                "folder".into(),
                Action::OpenInEditor(PathBuf::new()),
            ),
        ];
        if !self.refs.head.is_empty() {
            items.push((
                "Create branch".into(),
                "branch".into(),
                Action::Dialog(Dialog::create_branch(&self.refs.head, "HEAD")),
            ));
            items.push((
                "Create tag".into(),
                "tag".into(),
                Action::Dialog(Dialog::tag(&self.refs.head, "HEAD", self.default_remote())),
            ));
        }
        for b in &self.refs.local {
            if !b.is_head {
                items.push((
                    format!("Check out {}", b.name),
                    "branch".into(),
                    Action::Checkout(b.name.clone()),
                ));
            }
        }
        if let Some(current) = &self.refs.current {
            for b in &self.refs.local {
                if !b.is_head {
                    items.push((
                        format!("Changes {} adds to {current}", b.name),
                        "compare".into(),
                        compare_action(current, &b.name, true),
                    ));
                    items.push((
                        format!("Compare {current} with {}", b.name),
                        "compare".into(),
                        compare_action(current, &b.name, false),
                    ));
                    items.push((
                        format!("Merge {} into {current}", b.name),
                        "merge".into(),
                        merge_action(&b.name, current),
                    ));
                    items.push((
                        format!("Rebase {current} onto {}", b.name),
                        "rebase".into(),
                        rebase_action(&b.name, current),
                    ));
                }
            }
        }
        for (remote, branches) in &self.refs.remotes {
            for b in branches {
                items.push((
                    format!("Check out {}", b.name),
                    remote.clone(),
                    Action::CheckoutRemote(b.name.clone()),
                ));
            }
        }
        items
    }
}

/// Compares branch (or commit) `base` with `target`; `since_fork`: only what `target` changed
/// since they diverged.
pub(crate) fn compare_action(base: &str, target: &str, since_fork: bool) -> Action {
    Action::Compare {
        base: base.to_owned(),
        base_label: base.to_owned(),
        target: Some(target.to_owned()),
        target_label: target.to_owned(),
        since_fork,
    }
}

pub(crate) fn merge_action(branch: &str, current: &str) -> Action {
    Action::UndoableOps {
        label: format!("Merge {branch} into {current}"),
        ops: ops(&[&["merge", "--no-edit", branch]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

/// Moves the current branch forward to `branch`; fails (and says so) if they have diverged.
pub(crate) fn fast_forward_action(branch: &str, current: &str) -> Action {
    Action::UndoableOps {
        label: format!("Fast-forward {current} to {branch}"),
        ops: ops(&[&["merge", "--ff-only", branch]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

pub(crate) fn rebase_action(onto: &str, current: &str) -> Action {
    Action::UndoableOps {
        label: format!("Rebase {current} onto {onto}"),
        ops: ops(&[&["rebase", onto]]),
        refresh: Refresh::All,
        undo: Undo::HeadMove { mode: "--keep" },
    }
}

fn abort_action(state: InProgress) -> Action {
    Action::Ops {
        label: format!("Abort {}", state.label().to_lowercase()),
        ops: ops(&[&[state.command(), "--abort"]]),
        refresh: Refresh::All,
    }
}

pub(crate) fn reveal_label() -> &'static str {
    if cfg!(windows) {
        "Show in Explorer"
    } else if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Open folder"
    }
}

/// Runs the user's editor command with `path` (through `cmd` on Windows, where editors are
/// usually `.cmd` scripts).
pub(crate) fn open_in_editor(editor: &str, path: &Path) -> Result<(), String> {
    let editor = if editor.trim().is_empty() {
        "code"
    } else {
        editor.trim()
    };
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(editor).arg(path);
        c
    } else {
        let mut c = std::process::Command::new(editor);
        c.arg(path);
        crate::shell_path::apply(&mut c);
        c
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not run {editor}: {e}"))
}

fn short_duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h", s / 3600),
    }
}

/// The line of a git error worth showing in the toolbar: conflicts and `error:`/`fatal:` lines
/// beat progress chatter like "Auto-merging x".
fn headline(msg: &str) -> String {
    let (label, body) = msg.split_once(": ").unwrap_or(("", msg));
    let conflicts = body.lines().filter(|l| l.starts_with("CONFLICT")).count();
    if conflicts > 0 {
        let files = if conflicts == 1 { "file" } else { "files" };
        return format!("{label}: conflicts in {conflicts} {files}, resolve them in Changes");
    }
    let line = body
        .lines()
        .find(|l| l.starts_with("error:") || l.starts_with("fatal:"))
        .or_else(|| body.lines().find(|l| !l.trim().is_empty()))
        .unwrap_or_default();
    if label.is_empty() {
        line.to_owned()
    } else {
        format!("{label}: {line}")
    }
}

/// A hash as the UI shows it: its first 8 characters.
pub(crate) fn short(hash: &str) -> &str {
    hash.get(..8).unwrap_or(hash)
}

/// `&[&["checkout", "main"]]` → owned argument lists.
pub(crate) fn ops(list: &[&[&str]]) -> Vec<Vec<String>> {
    list.iter()
        .map(|args| args.iter().map(|a| (*a).to_owned()).collect())
        .collect()
}

/// Runs `job` on a background thread and wakes the UI when its message is sent.
fn spawn(tx: &Sender<Msg>, ctx: &egui::Context, job: impl FnOnce() -> Msg + Send + 'static) {
    let tx = tx.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(job());
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::headline;

    #[test]
    fn headline_prefers_conflicts_and_errors() {
        let merge = "Merge x failed: Auto-merging a.rs
CONFLICT (content): Merge conflict in a.rs
Automatic merge failed";
        assert_eq!(
            headline(merge),
            "Merge x failed: conflicts in 1 file, resolve them in Changes"
        );
        let push = "Push failed: To origin
 ! [rejected] main
error: failed to push some refs";
        assert_eq!(
            headline(push),
            "Push failed: error: failed to push some refs"
        );
        assert_eq!(headline("Fetch: done"), "Fetch: done");
    }
}
