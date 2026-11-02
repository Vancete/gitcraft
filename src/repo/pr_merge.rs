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

pub(super) fn merge_box(ui: &mut Ui, draft: &mut PrDraft, d: &PrDetails, acts: &mut Vec<Action>) {
    let t = Tokens::get(ui.ctx());
    let extras = &d.extras;
    let blocker = pr_post::merge_blocker(d.draft, &d.mergeable, &d.merge_state, extras.can_write());
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
    if host_path.as_ref() == own_branch.as_ref() {
        let mut delete = draft.delete_branch.unwrap_or(extras.delete_branch_on_merge);
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
    if blocker.is_none() && (failed > 0 || running > 0) {
        ui.label(
            RichText::new(format!(
                "{failed} check(s) failed and {running} still running, but none is required."
            ))
            .color(t.warning)
            .small(),
        );
    }
    if let Some(blocker) = blocker {
        ui.label(RichText::new(blocker).color(t.text_faint).small());
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
        ui.horizontal(|ui| {
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
    } else if ui
        .add_enabled(enabled, egui::Button::new("Merge pull request"))
        .clicked()
    {
        acts.push(Action::Pr(PrAct::AskMerge));
    }
}
