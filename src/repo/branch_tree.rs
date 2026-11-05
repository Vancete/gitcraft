//! Names as a tree: `docs/accuracy/gearbox` is a branch in a folder in a folder, and
//! `src/repo/mod.rs` a changed file in one.
//!
//! Branch folders start collapsed, file folders open (those of all the files of a commit
//! collapsed). The ones changed are saved with the app.

use std::collections::HashMap;

use egui::{Align2, Id, Rect, Response, Sense, Ui, pos2, vec2};

use super::sidebar::ROW_H;
use super::widgets;
use crate::icons;
use crate::settings::Settings;
use crate::theme::{self, Tokens};

/// How much further in each level of folders is.
const INDENT: f32 = 14.0;

#[derive(Debug, PartialEq)]
pub(super) enum Node<'a, T> {
    Folder(Folder<'a, T>),
    Leaf { name: &'a str, item: T },
}

#[derive(Debug, PartialEq)]
pub(super) struct Folder<'a, T> {
    pub name: &'a str,
    /// The folder's whole path, which is what identifies it: `docs/accuracy`.
    pub path: &'a str,
    pub children: Vec<Node<'a, T>>,
}

impl<T> Folder<'_, T> {
    /// How many branches are in here, at any depth.
    pub fn leaves(&self) -> usize {
        self.children
            .iter()
            .map(|node| match node {
                Node::Folder(folder) => folder.leaves(),
                Node::Leaf { .. } => 1,
            })
            .sum()
    }

    /// Whether a branch in here, at any depth, satisfies `pred`.
    pub fn any(&self, pred: &impl Fn(&T) -> bool) -> bool {
        self.children.iter().any(|node| match node {
            Node::Folder(folder) => folder.any(pred),
            Node::Leaf { item, .. } => pred(item),
        })
    }
}

/// The tree of `(name, item)`. Siblings keep the order they first appear in, so a sorted list
/// stays sorted.
pub(super) fn build<'a, T>(items: impl IntoIterator<Item = (&'a str, T)>) -> Vec<Node<'a, T>> {
    let mut roots = Vec::new();
    // Where each folder is among its siblings, by its whole path: a folder can hold thousands
    // of files, and looking through them for each would be quadratic.
    let mut folders = HashMap::new();
    for (name, item) in items {
        insert(&mut roots, &mut folders, name, 0, item);
    }
    roots
}

/// Puts `item` into `nodes`, for the part of the name `full` that starts at `start`.
fn insert<'a, T>(
    nodes: &mut Vec<Node<'a, T>>,
    folders: &mut HashMap<&'a str, usize>,
    full: &'a str,
    start: usize,
    item: T,
) {
    let rest = &full[start..];
    // A slash at the end doesn't open a folder: git shows a nested repository as `sub/`.
    let Some(slash) = rest.find('/').filter(|&i| i + 1 < rest.len()) else {
        nodes.push(Node::Leaf { name: rest, item });
        return;
    };
    let path = &full[..start + slash];
    let index = *folders.entry(path).or_insert_with(|| {
        nodes.push(Node::Folder(Folder {
            name: &rest[..slash],
            path,
            children: Vec::new(),
        }));
        nodes.len() - 1
    });
    if let Some(Node::Folder(folder)) = nodes.get_mut(index) {
        insert(&mut folder.children, folders, full, start + slash + 1, item);
    }
}

/// What a branch's row needs from the tree.
#[derive(Clone, Copy)]
pub(super) struct LeafRow<'a> {
    /// The last part of the branch's name: its folders are shown above it.
    pub name: &'a str,
    pub indent: f32,
}

/// Merges each folder whose only child is a folder with it, `src/repo` instead of `src` holding
/// `repo`, and puts folders before what isn't one, at every level.
pub(super) fn compact<T>(nodes: &mut [Node<'_, T>]) {
    nodes.sort_by_key(|node| !matches!(node, Node::Folder(_)));
    for node in nodes {
        if let Node::Folder(folder) = node {
            while let [Node::Folder(_)] = folder.children.as_slice() {
                let Some(Node::Folder(child)) = folder.children.pop() else {
                    break;
                };
                let start = folder.path.len() - folder.name.len();
                folder.name = &child.path[start..];
                folder.path = child.path;
                folder.children = child.children;
            }
            compact(&mut folder.children);
        }
    }
}

/// How to show a tree.
#[derive(Clone, Copy)]
pub(super) struct TreeView {
    /// Identifies the tree among those of all repositories.
    pub id: Id,
    /// While a filter is on, every folder with a match is open and stays as it was saved.
    pub filtering: bool,
    /// Where the tree starts.
    pub indent: f32,
    /// Whether a folder nobody has opened or closed yet is open.
    pub start_open: bool,
    /// Where a top-level folder's chevron is, from the left of the row.
    pub left: f32,
    /// How tall a leaf's row is, when every one is as tall: rows out of sight are then only
    /// skipped over, not drawn.
    pub leaf_h: Option<f32>,
}

impl TreeView {
    /// Branches in the sidebar: folders stay closed until opened.
    pub fn branches(id: Id, filtering: bool, indent: f32) -> Self {
        Self {
            id,
            filtering,
            indent,
            start_open: false,
            left: 34.0,
            // A filter opens every folder with a match: thousands of rows.
            leaf_h: Some(ROW_H),
        }
    }
}

/// Files, flat or as a tree of their folders as the settings say. `row` draws a file, given how
/// far in it is in the tree (`None`: in a flat list, showing its folder); a folder holding the
/// file for which `selected` holds stands out while it is closed. `start_open`: whether a
/// folder nobody has opened or closed yet is open.
pub(super) fn files<'a, T>(
    ui: &mut Ui,
    (id, start_open): (Id, bool),
    files: &'a [T],
    path: impl Fn(&'a T) -> &'a str,
    selected: &impl Fn(&&'a T) -> bool,
    row: &mut impl FnMut(&mut Ui, Option<f32>, &'a T),
) {
    if !Settings::get(ui.ctx()).files_as_tree {
        // A commit can change tens of thousands of files: only the rows in sight are drawn.
        // While a menu is open (perhaps one of a row out of sight, which must go on being
        // shown), row by row.
        if egui::Popup::is_any_open(ui.ctx()) {
            for f in files {
                if !skip_unseen(ui, widgets::ROW_H) {
                    row(ui, None, f);
                }
            }
            return;
        }
        let pitch = widgets::ROW_H + ui.spacing().item_spacing.y;
        let top = ui.cursor().top();
        let clip = ui.clip_rect();
        let first = (((clip.top() - top) / pitch).floor().max(0.0) as usize).min(files.len());
        let end = ((((clip.bottom() - top) / pitch).ceil().max(0.0) as usize) + 1)
            .clamp(first, files.len());
        if first > 0 {
            ui.add_space(first as f32 * pitch);
            // The rows in sight keep the ids they would have with every row drawn.
            ui.skip_ahead_auto_ids(first);
        }
        for f in &files[first..end] {
            row(ui, None, f);
        }
        if end < files.len() {
            ui.add_space((files.len() - end) as f32 * pitch);
            // What follows the list (another list) keeps its ids too.
            ui.skip_ahead_auto_ids(files.len() - end);
        }
        return;
    }
    let mut tree = build(files.iter().map(|f| (path(f), f)));
    compact(&mut tree);
    let view = TreeView {
        id,
        filtering: false,
        indent: 0.0,
        start_open,
        left: 18.0,
        leaf_h: Some(widgets::ROW_H),
    };
    show(ui, &tree, &view, selected, &mut |ui, leaf, f| {
        row(ui, Some(leaf.indent), f);
    });
}

/// The panel-header switch that shows files as a flat list of their paths or as a tree of their
/// folders.
pub(super) fn files_toggle(ui: &mut Ui) {
    let mut settings = Settings::get(ui.ctx());
    let on = usize::from(settings.files_as_tree);
    if let Some(picked) = widgets::switch(ui, &["Path", "Tree"], on) {
        settings.files_as_tree = picked == 1;
        settings.set(ui.ctx());
    }
}

/// Shows the rows of a tree, with `leaf` drawing the branches. A folder with a branch for which
/// `is_head` holds stands out, so that a collapsed tree still shows where HEAD is.
pub(super) fn show<'a, T>(
    ui: &mut Ui,
    nodes: &[Node<'a, T>],
    view: &TreeView,
    is_head: &impl Fn(&T) -> bool,
    leaf: &mut impl FnMut(&mut Ui, LeafRow<'a>, &T),
) {
    for node in nodes {
        match node {
            Node::Leaf { .. } if view.leaf_h.is_some_and(|h| skip_unseen(ui, h)) => {}
            Node::Leaf { name, item } => {
                let row = LeafRow {
                    name,
                    indent: view.indent,
                };
                leaf(ui, row, item);
            }
            Node::Folder(folder) => {
                let id = view.id.with(folder.path);
                // Only the folders that differ from how they start are kept.
                let saved = ui
                    .data_mut(|d| d.get_persisted::<bool>(id))
                    .unwrap_or(false);
                let open = view.filtering || saved != view.start_open;
                let toggled = !skip_unseen(ui, ROW_H)
                    && folder_row(ui, folder, view, open, folder.any(is_head)).clicked()
                    && !view.filtering;
                if toggled {
                    ui.data_mut(|d| {
                        if saved {
                            d.remove::<bool>(id);
                        } else {
                            d.insert_persisted(id, true);
                        }
                    });
                }
                if open != toggled {
                    let inner = TreeView {
                        indent: view.indent + INDENT,
                        ..*view
                    };
                    show(ui, &folder.children, &inner, is_head, leaf);
                }
            }
        }
    }
}

/// Moves past a row `h` tall instead of drawing it, when it would be out of sight. The rows
/// after it keep the ids they would have had. A row whose menu is open is drawn anyway: a
/// menu not shown for a frame closes.
pub(super) fn skip_unseen(ui: &mut Ui, h: f32) -> bool {
    let rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), h));
    // The row's response would get the next id, and its menu that id's (see
    // `egui::Popup::default_response_id`).
    let menu = ui.next_auto_id().with("popup");
    if ui.is_rect_visible(rect) || egui::Popup::is_id_open(ui.ctx(), menu) {
        return false;
    }
    ui.add_space(h + ui.spacing().item_spacing.y);
    ui.skip_ahead_auto_ids(1);
    true
}

/// A folder: chevron where its branches have their icon, name, how many branches it holds.
fn folder_row<T>(
    ui: &mut Ui,
    folder: &Folder<'_, T>,
    view: &TreeView,
    open: bool,
    has_head: bool,
) -> Response {
    let t = Tokens::get(ui.ctx());
    let x = view.left + view.indent;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let painter = ui.painter_at(rect);
    if resp.hovered() {
        painter.rect_filled(rect, 0.0, t.hover);
    }
    let y = rect.center().y;
    icons::paint(
        ui,
        Rect::from_center_size(pos2(rect.left() + x, y), vec2(14.0, 14.0)),
        if open {
            "chevron-down"
        } else {
            "chevron-right"
        },
        12.0,
        if has_head { t.accent } else { t.text_faint },
    );
    let count = painter.text(
        pos2(rect.right() - 10.0, y),
        Align2::RIGHT_CENTER,
        folder.leaves().to_string(),
        theme::medium(11.0),
        t.text_faint,
    );
    let font = if has_head {
        theme::semibold(12.5)
    } else {
        egui::FontId::proportional(12.5)
    };
    painter
        .with_clip_rect(rect.with_max_x(count.left() - 6.0))
        .text(
            pos2(rect.left() + x + 14.0, y),
            Align2::LEFT_CENTER,
            folder.name,
            font,
            t.text,
        );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(name: &str, item: u32) -> Node<'_, u32> {
        Node::Leaf { name, item }
    }

    fn folder<'a>(name: &'a str, path: &'a str, children: Vec<Node<'a, u32>>) -> Node<'a, u32> {
        Node::Folder(Folder {
            name,
            path,
            children,
        })
    }

    fn only_folder<'a, 'b>(nodes: &'b [Node<'a, u32>]) -> Option<&'b Folder<'a, u32>> {
        match nodes {
            [Node::Folder(folder)] => Some(folder),
            _ => None,
        }
    }

    #[test]
    fn names_without_a_slash_stay_flat() {
        assert_eq!(
            build([("main", 1), ("dev", 2)]),
            vec![leaf("main", 1), leaf("dev", 2)]
        );
    }

    #[test]
    fn no_branches_make_no_nodes() {
        assert_eq!(build::<u32>([]), vec![]);
    }

    #[test]
    fn a_shared_prefix_becomes_one_folder() {
        assert_eq!(
            build([("ci/coverage", 1), ("ci/hardening", 2)]),
            vec![folder(
                "ci",
                "ci",
                vec![leaf("coverage", 1), leaf("hardening", 2)]
            )]
        );
    }

    #[test]
    fn folders_nest_and_know_their_whole_path() {
        assert_eq!(
            build([("docs/accuracy/gearbox", 1)]),
            vec![folder(
                "docs",
                "docs",
                vec![folder(
                    "accuracy",
                    "docs/accuracy",
                    vec![leaf("gearbox", 1)]
                )]
            )]
        );
    }

    #[test]
    fn siblings_keep_the_order_they_first_appear_in() {
        assert_eq!(
            build([("chore/a", 1), ("main", 2), ("t3/b", 3)]),
            vec![
                folder("chore", "chore", vec![leaf("a", 1)]),
                leaf("main", 2),
                folder("t3", "t3", vec![leaf("b", 3)]),
            ]
        );
    }

    #[test]
    fn a_folder_collects_names_that_are_not_next_to_each_other() {
        assert_eq!(
            build([("ci/a", 1), ("main", 2), ("ci/b", 3)]),
            vec![
                folder("ci", "ci", vec![leaf("a", 1), leaf("b", 3)]),
                leaf("main", 2),
            ]
        );
    }

    #[test]
    fn folders_of_the_same_name_in_different_places_stay_apart() {
        assert_eq!(
            build([("a/x/1", 1), ("b/x/2", 2)]),
            vec![
                folder("a", "a", vec![folder("x", "a/x", vec![leaf("1", 1)])]),
                folder("b", "b", vec![folder("x", "b/x", vec![leaf("2", 2)])]),
            ]
        );
    }

    #[test]
    fn a_trailing_slash_names_a_leaf_not_an_empty_folder() {
        assert_eq!(
            build([("sub/", 1), ("lib/nested/", 2)]),
            vec![
                leaf("sub/", 1),
                folder("lib", "lib", vec![leaf("nested/", 2)]),
            ]
        );
    }

    #[test]
    fn a_name_and_a_folder_of_the_same_name_stay_apart() {
        // A file replaced by a folder of the same name, in one commit.
        assert_eq!(
            build([("a", 1), ("a/b", 2)]),
            vec![leaf("a", 1), folder("a", "a", vec![leaf("b", 2)])]
        );
    }

    #[test]
    fn unicode_names_split_at_their_slashes() {
        assert_eq!(
            build([("ünï/çødé/日本.rs", 1)]),
            vec![folder(
                "ünï",
                "ünï",
                vec![folder("çødé", "ünï/çødé", vec![leaf("日本.rs", 1)])]
            )]
        );
        assert_eq!(
            compacted(&[("ünï/çødé/日本.rs", 1)]),
            vec![folder("ünï/çødé", "ünï/çødé", vec![leaf("日本.rs", 1)])]
        );
    }

    #[test]
    fn a_folder_counts_the_branches_at_any_depth() {
        let tree = build([("docs/a", 1), ("docs/deep/b", 2), ("docs/deep/c", 3)]);
        assert_eq!(only_folder(&tree).map(Folder::leaves), Some(3));
    }

    #[test]
    fn a_folder_finds_a_branch_at_any_depth() {
        let tree = build([("docs/a", 1), ("docs/deep/b", 2)]);
        let has = |wanted: u32| only_folder(&tree).map(|f| f.any(&|item| *item == wanted));
        assert_eq!(has(2), Some(true));
        assert_eq!(has(9), Some(false));
    }

    fn compacted<'a>(items: &[(&'a str, u32)]) -> Vec<Node<'a, u32>> {
        let mut tree = build(items.iter().copied());
        compact(&mut tree);
        tree
    }

    #[test]
    fn a_chain_of_lone_folders_becomes_one() {
        assert_eq!(
            compacted(&[("src/repo/mod.rs", 1), ("src/repo/diff.rs", 2)]),
            vec![folder(
                "src/repo",
                "src/repo",
                vec![leaf("mod.rs", 1), leaf("diff.rs", 2)]
            )]
        );
    }

    #[test]
    fn a_folder_with_files_of_its_own_is_not_merged() {
        assert_eq!(
            compacted(&[("src/lib.rs", 1), ("src/repo/mod.rs", 2)]),
            vec![folder(
                "src",
                "src",
                vec![
                    folder("repo", "src/repo", vec![leaf("mod.rs", 2)]),
                    leaf("lib.rs", 1)
                ]
            )]
        );
    }

    #[test]
    fn merging_stops_where_folders_branch() {
        assert_eq!(
            compacted(&[("a/b/c/1", 1), ("a/b/d/2", 2)]),
            vec![folder(
                "a/b",
                "a/b",
                vec![
                    folder("c", "a/b/c", vec![leaf("1", 1)]),
                    folder("d", "a/b/d", vec![leaf("2", 2)]),
                ]
            )]
        );
    }

    #[test]
    fn folders_come_before_files_keeping_their_order() {
        assert_eq!(
            compacted(&[
                ("Cargo.toml", 1),
                ("src/a", 2),
                ("README.md", 3),
                ("docs/b", 4)
            ]),
            vec![
                folder("src", "src", vec![leaf("a", 2)]),
                folder("docs", "docs", vec![leaf("b", 4)]),
                leaf("Cargo.toml", 1),
                leaf("README.md", 3),
            ]
        );
    }
}
