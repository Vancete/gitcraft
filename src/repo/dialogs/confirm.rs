//! The dialog that asks before a destructive operation runs.

use egui::{RichText, Ui};

use crate::repo::{Action, Refresh, Undo};
use crate::theme::{self, Tokens};

use super::{Outcome, cancel_button};

/// Asks before running `ops`, or the `alternative` ones when its box is ticked.
#[allow(clippy::too_many_arguments)]
pub(super) fn confirm(
    ui: &mut Ui,
    title: &str,
    body: &str,
    button: &str,
    ops: &[Vec<String>],
    alternative: &Option<(String, Vec<Vec<String>>)>,
    use_alternative: &mut bool,
    refresh: Refresh,
    undo: &Undo,
) -> Outcome {
    let t = Tokens::get(ui.ctx());
    ui.label(RichText::new(title).font(theme::semibold(15.0)));
    ui.add_space(6.0);
    ui.add(egui::Label::new(body).wrap());
    if let Some((label, _)) = alternative {
        ui.add_space(6.0);
        ui.checkbox(use_alternative, label.as_str());
    }
    ui.add_space(14.0);
    let mut outcome = Outcome::Open;
    ui.horizontal(|ui| {
        let danger = egui::Button::new(
            RichText::new(button)
                .color(egui::Color32::WHITE)
                .font(theme::medium(12.5)),
        )
        .fill(t.danger)
        .min_size(egui::vec2(90.0, 30.0));
        if ui.add(danger).clicked() {
            let steps = match (alternative, *use_alternative) {
                (Some((_, alt)), true) => alt.clone(),
                _ => ops.to_vec(),
            };
            outcome = Outcome::Done(match undo {
                Undo::None => Action::Ops {
                    label: title.to_owned(),
                    ops: steps,
                    refresh,
                },
                undo => Action::UndoableOps {
                    label: title.to_owned(),
                    ops: steps,
                    refresh,
                    undo: undo.clone(),
                },
            });
        }
        if cancel_button(ui) {
            outcome = Outcome::Cancel;
        }
    });
    outcome
}
