//! One open repository: its state, background git work and the view
//! (branches on the left, commit graph or diff in the middle, commit detail / changes on the right).

mod actions;
mod apply;
mod branch_tree;
mod branch_visibility;
mod ci_status;
mod ci_view;
pub(crate) mod columns;
mod commands;
mod conflict;
mod detail;
mod dialogs;
mod diff;
mod file_text;
mod file_view;
mod graph_view;
mod highlight;
mod keep_submodules;
mod layout;
mod load;
mod messages;
mod navigate;
mod operations;
mod patches;
mod pinned_sections;
mod pr_merge;
mod pr_review;
mod pr_view;
pub(crate) mod pull;
mod quick_edit;
mod rebase;
mod rewrite;
mod sidebar;
mod state;
mod terminals;
#[cfg(test)]
mod tests;
mod toolbar;
mod watch;
pub(crate) mod widgets;
pub(crate) mod worktree_tab;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use crate::forge::{Forge, PullRequest};
use crate::git::{
    self, Commit, CommitDetail, Identity, Refs, Remote, Stash, Submodule, WorkStatus, Worktree,
};
use crate::graph::Graph;
use crate::terminal::Terminal;

use actions::abort_action;
pub(crate) use actions::{
    Action, compare_action, fast_forward_action, merge_action, rebase_action,
};
pub use dialogs::Dialog;
use messages::{Msg, spawn};
pub(crate) use state::{Compare, CompareData, DiffKey, DiffView, Refresh, Reword, Undo, UndoEntry};
use state::{OpKind, PendingOp, Search};
pub(crate) use toolbar::reveal_label;
pub use widgets::primary_button;

/// Hash of the pseudo-commit that stands for uncommitted changes at the top of the graph.
pub(crate) const WIP_HASH: &str = "\u{0}wip";
/// History deeper than this isn't loaded by scrolling or searching.
const MAX_HISTORY: usize = 100_000;

/// How a tab asks the app to open a folder.
pub enum OpenRequest {
    NewTab(PathBuf),
    /// In the same tab or a new one, as the settings say.
    Worktree(PathBuf),
}

pub struct RepoTab {
    pub path: PathBuf,
    pub name: String,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    loading: bool,
    /// A reload was asked for while one was running, which may have read too early.
    reload_again: bool,
    load_error: Option<String>,
    refs: Refs,
    /// Commits as git listed them.
    history: Vec<Commit>,
    has_more: bool,
    loading_more: bool,
    /// The branches hidden from the graph or soloed in it.
    visibility: branch_visibility::Visibility,
    /// What `history` was logged from: `None` for every branch.
    log_from: Option<Vec<String>>,
    /// Counts the reloads of `history`, so that a page asked for before one is dropped.
    history_gen: u64,
    work: WorkStatus,
    /// Rows of the graph: the WIP pseudo-commit (when the tree is dirty) followed by `history`.
    commits: Vec<Commit>,
    graph: Graph,
    index: HashMap<String, usize>,
    worktrees: Vec<Worktree>,
    stashes: Vec<Stash>,
    remotes: Vec<Remote>,
    submodules: Vec<Submodule>,
    identity: Identity,
    forge: Option<Forge>,
    prs: Vec<PullRequest>,
    pr_error: Option<String>,
    prs_loaded: Option<Instant>,
    /// The build status of the commits that show one.
    ci: ci_status::CiStatuses,

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
    /// The pull request the big area shows.
    pr_view: Option<pr_view::PrView>,
    /// The builds of a commit the big area shows.
    ci_view: Option<ci_view::CiView>,
    pr_requests: u64,
    /// What was written on each pull request and not sent, by its number.
    pr_drafts: HashMap<u64, pr_review::PrDraft>,
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
    diff_ignore_ws: bool,
    /// Diffs show the whole file instead of the changes.
    diff_as_file: bool,
    /// Whether the commit pane lists every file of the commit, and those read.
    all_files: detail::AllFiles,
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
    /// The running operation moves the checkout: update the submodules after it, if asked to.
    update_submodules: bool,
    status: Option<(String, bool)>,
    branch_filter: String,
    /// Where a clicked jump mark wants the sidebar scrolled to.
    sidebar_jump: Option<f32>,
    /// A sidebar section to open and scroll to, asked for from the folded sidebar.
    sidebar_reveal: Option<&'static str>,
    /// Put the cursor in the branch filter on the next frame.
    focus_branch_filter: bool,
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
    /// A folder the app should open (taken by `App`).
    pub open_request: Option<OpenRequest>,
    /// The other worktrees this tab was on, oldest first, for coming back to. Boxed as in
    /// `Tab::Repo`, which they move in and out of.
    #[allow(clippy::vec_box)]
    parked: Vec<Box<RepoTab>>,
    /// Those of them with work going on, and what work.
    parked_status: Vec<(PathBuf, &'static str)>,
    /// The app has a dialog of its own open (palette, settings): leave the keys to it.
    pub keys_blocked: bool,
    /// Long operations that just ended, its own and its parked worktrees': (message, failed),
    /// for a notification each (taken by `App`).
    pub finished: Vec<(String, bool)>,
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
            reload_again: false,
            load_error: None,
            refs: Refs::default(),
            history: Vec::new(),
            has_more: false,
            loading_more: false,
            visibility: Default::default(),
            log_from: None,
            history_gen: 0,
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
            submodules: Vec::new(),
            identity: Identity::default(),
            forge: None,
            prs: Vec::new(),
            pr_error: None,
            prs_loaded: None,
            ci: ci_status::CiStatuses::default(),
            selected: None,
            stash_view: None,
            scroll_to: None,
            detail_hash: None,
            detail: None,
            detail_error: None,
            diff: None,
            compare: None,
            pr_view: None,
            ci_view: None,
            pr_requests: 0,
            pr_drafts: HashMap::new(),
            compare_requests: 0,
            file_view: None,
            rebase: None,
            reword: None,
            credentials: None,
            last_op: None,
            avatars_hinted: false,
            diff_ignore_ws: false,
            diff_as_file: false,
            all_files: Default::default(),
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
            update_submodules: false,
            status: None,
            branch_filter: String::new(),
            sidebar_jump: None,
            sidebar_reveal: None,
            focus_branch_filter: false,
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
            parked: Vec::new(),
            parked_status: Vec::new(),
            keys_blocked: false,
            finished: Vec::new(),
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

    /// HTTPS credentials for this tab's git commands (from the clone that made it).
    pub fn set_credentials(&mut self, creds: git::Credentials) {
        self.credentials = Some(creds);
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
