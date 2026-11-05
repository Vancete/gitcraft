//! The box on a pull request's page that merges it.

use egui::{RichText, Ui};

use super::Action;
use super::pr_review::{Place, PrAct, PrDraft};
use crate::pr_post::{self, MergeMethod};
use crate::pull_request::PrDetails;
use crate::theme::Tokens;

/// The merge method to offer first: the one chosen, or the first this repository allows.
fn method_of(draft: &PrDraft, d: &PrDetails) -> MergeMethod {
    let extras = &d.extras;
    let allowed = |method: MergeMethod| match method {
        MergeMethod::Merge => extras.merge_allowed,
        MergeMethod::Squash => extras.squash_allowed,
        MergeMethod::Rebase => extras.rebase_allowed,
    };
    draft
        .merge_method
        .filter(|m| allowed(*m))
        .or_else(|| {
            [MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase]
                .into_iter()
                .find(|m| allowed(*m))
        })
        .unwrap_or_default()
}

/// Whether the app has to delete the branch after merging. Where the repository is set to do it,
/// GitHub does so whatever is chosen here.
pub(super) fn deletes_branch(own_branch: bool, repo_deletes: bool, choice: Option<bool>) -> bool {
    own_branch && !repo_deletes && choice == Some(true)
}

/// What to say of checks that failed or still run when it can be merged. Past the rules (a
/// blocked pull request merged as an admin) they may well be required ones.
fn checks_note(blocked: bool, merge_state: &str, failed: usize, running: usize) -> Option<String> {
    if blocked || (failed == 0 && running == 0) {
        return None;
    }
    let required = if merge_state == "BLOCKED" {
        "some may be required"
    } else {
        "none is required"
    };
    Some(format!(
        "{failed} check(s) failed and {running} still running, but {required}."
    ))
}

pub(super) fn merge_box(ui: &mut Ui, draft: &mut PrDraft, d: &PrDetails, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    let extras = &d.extras;
    if extras.pr_id.is_empty() {
        // Nothing is known about the rights or the merge methods, so nothing can be said of them.
        ui.label(
            RichText::new("Couldn't read what can be done here. Load it again.")
                .color(t.warning)
                .small(),
        );
        return;
    }
    let blocker = pr_post::merge_blocker(
        d.draft,
        &d.mergeable,
        &d.merge_state,
        extras.can_write(),
        extras.can_bypass_rules,
    );
    let method = method_of(draft, d);
    draft.merge_method = Some(method);
    ui.horizontal_wrapped(|ui| {
        for (option, allowed, label) in [
            (MergeMethod::Merge, extras.merge_allowed, "Merge commit"),
            (MergeMethod::Squash, extras.squash_allowed, "Squash"),
            (MergeMethod::Rebase, extras.rebase_allowed, "Rebase"),
        ] {
            if allowed && ui.radio(method == option, label).clicked() {
                draft.merge_method = Some(option);
            }
        }
    });
    // Only a branch of this repository is this repository's to delete.
    let (host_path, own_branch) = (
        crate::pull_request::repo_of_url(&d.url).map(|(_, path)| path),
        &extras.head_repo,
    );
    if host_path.as_ref() == own_branch.as_ref() && extras.delete_branch_on_merge {
        ui.label(
            RichText::new(format!(
                "GitHub deletes {} afterwards: this repository is set to.",
                d.head_branch
            ))
            .color(t.text_faint)
            .small(),
        );
    } else if host_path.as_ref() == own_branch.as_ref() {
        let mut delete = draft.delete_branch.unwrap_or(false);
        if ui
            .checkbox(&mut delete, format!("Delete {} afterwards", d.head_branch))
            .changed()
        {
            draft.delete_branch = Some(delete);
        }
    }
    if let Some(error) = draft.error_at(Place::Merge) {
        ui.colored_label(t.danger, error);
    }
    let (_, failed, running, _) = crate::pull_request::tally(&d.checks);
    if let Some(note) = checks_note(blocker.is_some(), &d.merge_state, failed, running) {
        ui.label(RichText::new(note).color(t.warning).small());
    }
    if let Some(blocker) = blocker {
        ui.label(RichText::new(blocker).color(t.text_faint).small());
    } else if d.merge_state == "BLOCKED" {
        ui.label(
            RichText::new("The repository's rules aren't met, but you may merge past them.")
                .color(t.warning)
                .small(),
        );
    }
    let enabled = blocker.is_none() && !draft.posting;
    if draft.confirm_merge {
        ui.label(
            RichText::new(format!(
                "Merge #{} into {}? Everyone sees this at once, and it can't be undone from here.",
                d.number, d.base_branch
            ))
            .color(t.warning),
        );
        let buttons = ui.horizontal(|ui| {
            if ui
                .add_enabled(enabled, egui::Button::new("Yes, merge"))
                .clicked()
            {
                acts.push(Action::Pr(PrAct::Merge));
            }
            if ui.button("Cancel").clicked() {
                acts.push(Action::Pr(PrAct::CancelMerge));
            }
            if draft.posting {
                ui.spinner();
            }
        });
        if std::mem::take(&mut draft.reveal_merge) {
            buttons.response.scroll_to_me(Some(egui::Align::BOTTOM));
        }
    } else if ui
        .add_enabled(enabled, egui::Button::new("Merge pull request"))
        .clicked()
    {
        acts.push(Action::Pr(PrAct::AskMerge));
    }
}

#[cfg(test)]
mod tests {
    use super::checks_note;

    #[test]
    fn checks_past_the_rules_are_not_said_to_be_optional() {
        assert_eq!(checks_note(false, "CLEAN", 0, 0), None);
        assert_eq!(checks_note(true, "UNSTABLE", 1, 0), None);
        assert_eq!(
            checks_note(false, "UNSTABLE", 1, 2).as_deref(),
            Some("1 check(s) failed and 2 still running, but none is required.")
        );
        assert!(
            checks_note(false, "BLOCKED", 1, 0).is_some_and(|n| !n.contains("none is required"))
        );
    }
}
