//! Text of the pull request, Markdown and tables of images, wrapped to the page.

use egui::{RichText, Ui};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::pull_request::{self as pr, Block};
use crate::theme::{self, Tokens};

const TABLE_MARGIN: f32 = 8.0;

/// Text of the pull request, wrapped to `width`.
pub(in crate::repo) fn blocks(
    ui: &mut Ui,
    cache: &mut CommonMarkCache,
    id: &str,
    blocks: &[Block],
    width: f32,
) {
    ui.push_id(id, |ui| {
        for (i, block) in blocks.iter().enumerate() {
            ui.push_id(i, |ui| match block {
                Block::Markdown(text) => markdown(ui, cache, text, width),
                Block::ImageTable { header, rows } => image_table(ui, cache, header, rows, width),
            });
        }
    });
}

/// Text from strangers may link anywhere: a click only ever opens a web page. Called after
/// drawing such text, this drops anything else a link asked to open.
pub(in crate::repo) fn only_web_links(ctx: &egui::Context) {
    ctx.output_mut(|o| {
        o.commands.retain(
            |c| !matches!(c, egui::OutputCommand::OpenUrl(open) if !pr::is_web_url(&open.url)),
        );
    });
}

fn markdown(ui: &mut Ui, cache: &mut CommonMarkCache, text: &str, width: f32) {
    ui.set_max_width(width);
    // The viewer wraps text to at least this, so it can't be more than there is room for.
    CommonMarkViewer::new()
        .max_image_width(Some(width as usize))
        .show(ui, cache, text);
}

/// A table with images, its columns sharing `width` evenly.
fn image_table(
    ui: &mut Ui,
    cache: &mut CommonMarkCache,
    header: &[String],
    rows: &[Vec<String>],
    width: f32,
) {
    let t = Tokens::get(ui.ctx());
    let columns = rows
        .iter()
        .map(Vec::len)
        .fold(header.len(), usize::max)
        .max(1);
    let inner = width - 2.0 * TABLE_MARGIN;
    let gap = ui.spacing().item_spacing.x;
    let column_width = (inner - gap * (columns - 1) as f32) / columns as f32;
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, t.separator))
        .corner_radius(t.radius)
        .inner_margin(TABLE_MARGIN)
        .show(ui, |ui| {
            ui.set_width(inner);
            ui.columns(columns, |cells| {
                for (cell, title) in cells.iter_mut().zip(header) {
                    cell.label(RichText::new(title).font(theme::semibold(12.0)));
                }
            });
            for (r, row) in rows.iter().enumerate() {
                ui.add_space(6.0);
                ui.columns(columns, |cells| {
                    for (c, (cell, text)) in cells.iter_mut().zip(row).enumerate() {
                        cell.push_id((r, c), |ui| markdown(ui, cache, text, column_width));
                    }
                });
            }
        });
    ui.add_space(6.0);
}
