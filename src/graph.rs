//! Lane layout for the commit graph.
//!
//! Commits arrive newest first. Each lane "waits" for a commit hash; a commit takes the lane that
//! waits for it (or a free one if it's a branch tip), then hands the lane to its first parent and
//! opens or joins lanes for its other parents. Lanes keep their column for their whole life, so
//! lines that pass through a row are always vertical.

use std::borrow::Borrow;

use crate::git::Commit;

#[derive(Clone, Debug, Default)]
pub struct Row {
    /// Column of the commit's dot.
    pub col: usize,
    /// Lanes drawn straight through the row, top to bottom.
    pub through: Vec<usize>,
    /// Lanes that come from the top edge into the dot (children of this commit).
    pub incoming: Vec<usize>,
    /// Lanes that leave the dot towards the bottom edge (parents of this commit).
    pub outgoing: Vec<usize>,
}

pub struct Graph {
    pub rows: Vec<Row>,
    /// Widest the graph gets, in lanes.
    pub width: usize,
}

pub fn layout<C: Borrow<Commit>>(commits: &[C]) -> Graph {
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut width = 0;

    for commit in commits {
        let commit: &Commit = commit.borrow();
        let hash = commit.hash.as_str();
        let incoming: Vec<usize> = (0..lanes.len())
            .filter(|&i| lanes[i] == Some(hash))
            .collect();
        let col = match incoming.first() {
            Some(&c) => c,
            None => free_lane(&mut lanes),
        };
        let through: Vec<usize> = (0..lanes.len())
            .filter(|&i| lanes[i].is_some() && lanes[i] != Some(hash))
            .collect();
        for &i in &incoming {
            lanes[i] = None;
        }

        let mut outgoing = Vec::new();
        for (n, parent) in commit.parents.iter().enumerate() {
            let parent = parent.as_str();
            if n == 0 {
                // The first parent continues this lane. If another lane already waits for the
                // same parent, both run down and meet at the parent's dot (leftmost lane wins).
                lanes[col] = Some(parent);
                outgoing.push(col);
            } else if let Some(existing) = lanes.iter().position(|l| *l == Some(parent)) {
                outgoing.push(existing);
            } else {
                let lane = free_lane(&mut lanes);
                lanes[lane] = Some(parent);
                outgoing.push(lane);
            }
        }

        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        width = width.max(lanes.len()).max(col + 1);
        rows.push(Row {
            col,
            through,
            incoming,
            outgoing,
        });
    }

    Graph { rows, width }
}

fn free_lane(lanes: &mut Vec<Option<&str>>) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hash: &str, parents: &[&str]) -> Commit {
        Commit {
            hash: hash.into(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: String::new(),
            date: String::new(),
            summary: String::new(),
            refs: Vec::new(),
            ..Commit::default()
        }
    }

    #[test]
    fn linear_history_uses_one_lane() {
        let g = layout(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
        assert_eq!(g.width, 1);
        assert!(g.rows.iter().all(|r| r.col == 0));
    }

    #[test]
    fn merge_and_branch() {
        // m merges f into b; f and b both come from a.
        let g = layout(&[
            c("m", &["b", "f"]),
            c("f", &["a"]),
            c("b", &["a"]),
            c("a", &[]),
        ]);
        assert_eq!(g.width, 2);
        assert_eq!(g.rows[0].outgoing, vec![0, 1]);
        assert_eq!(g.rows[1].col, 1);
        assert_eq!(g.rows[1].through, vec![0]);
        assert_eq!(g.rows[2].col, 0);
        assert_eq!(g.rows[2].outgoing, vec![0]);
        // Both lanes wait for `a` and meet at its dot, in the leftmost lane.
        assert_eq!(g.rows[3].incoming, vec![0, 1]);
        assert_eq!(g.rows[3].col, 0);
    }
}
