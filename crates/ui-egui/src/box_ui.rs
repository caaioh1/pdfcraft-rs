//! Edit a PDF ▸ Edit text & images: the boxes a page draws (a shaded note, a framed paragraph, a
//! highlight). Right-click one for **Delete box** or **Delete box and its text**. The page's
//! own paragraph and picture handling is unchanged: a right-click that lands inside a box opens
//! this menu instead of the picture's, and nothing else about selecting or typing differs.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{DrawnBox, Edit};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, PageXform};

const ACCENT: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

/// The boxes of `page` in screen space, with their indexes.
fn screen_boxes(xf: &PageXform, info: &DocInfo, page: usize, boxes: &[DrawnBox]) -> Vec<(usize, Rect)> {
    if info.pages.get(page).is_none() {
        return Vec::new();
    }
    boxes.iter().enumerate().map(|(i, b)| (i, xf.user_rect(info, page, b.rect.map(|v| v as f32)))).filter(|(_, r)| r.is_positive()).collect()
}

/// The innermost box under `p`: the smallest one that contains it, so a highlight inside a
/// shaded note is the highlight and the rest of the note is the note.
pub(crate) fn box_at(xf: &PageXform, info: &DocInfo, page: usize, boxes: &[DrawnBox], p: Pos2) -> Option<usize> {
    screen_boxes(xf, info, page, boxes)
        .into_iter()
        .filter(|(_, r)| r.expand(1.0).contains(p))
        .min_by(|(_, a), (_, b)| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
        .map(|(i, _)| i)
}

/// A faint outline around the box the pointer is over, so a right-click knows what it will take.
pub(crate) fn paint_hover(ui: &egui::Ui, xf: &PageXform, info: &DocInfo, page: usize, boxes: &[DrawnBox], hover: Option<Pos2>) {
    let Some(p) = hover.filter(|p| xf.rect.contains(*p)) else { return };
    if let Some(i) = box_at(xf, info, page, boxes, p)
        && let Some((_, r)) = screen_boxes(xf, info, page, boxes).into_iter().find(|(j, _)| *j == i)
    {
        ui.painter().rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, ACCENT.gamma_multiply(0.55)), egui::StrokeKind::Outside);
    }
}

/// Edit text & images: the boxes page `page` draws. A right-click inside one opens the box menu
/// rather than a picture's, and the box under the pointer is outlined. Whether the box owns the
/// pointer this frame (the picture and text underneath then wait).
#[allow(clippy::too_many_arguments)]
pub(crate) fn page_input(
    ui: &egui::Ui,
    resp: &egui::Response,
    xf: &PageXform,
    info: &DocInfo,
    page: usize,
    doc: &pdfcraft_engine::Document,
    active: bool,
    pointer: Option<Pos2>,
    view: &mut DocView,
) -> bool {
    let boxes = if active {
        let generation = doc.edit_generation();
        match view.edit_boxes.get(&page) {
            Some((g, b)) if *g == generation => b.clone(),
            _ => {
                let b = doc.drawn_boxes(page).unwrap_or_default();
                view.edit_boxes.insert(page, (generation, b.clone()));
                b
            }
        }
    } else {
        Vec::new()
    };
    if !boxes.is_empty() {
        if resp.secondary_clicked()
            && let Some(p) = pointer.filter(|p| xf.rect.contains(*p))
            && let Some(index) = box_at(xf, info, page, &boxes, p)
        {
            view.box_context = Some((page, index));
        }
        paint_hover(ui, xf, info, page, &boxes, pointer);
    }
    view.box_context.is_some_and(|(p, _)| p == page) && (resp.secondary_clicked() || resp.context_menu_opened())
}

/// The menu of box `index` on `page`.
pub(crate) fn menu(ui: &mut egui::Ui, page: usize, index: usize, can_modify: bool, view: &mut DocView) {
    if ui.add_enabled(can_modify, egui::Button::new(tl!("Delete box"))).clicked() {
        view.pending_edit = Some(Edit::DeleteDrawnBox { page, index, with_text: false });
        view.box_context = None;
        ui.close();
    }
    if ui.add_enabled(can_modify, egui::Button::new(tl!("Delete box and its text"))).clicked() {
        view.pending_edit = Some(Edit::DeleteDrawnBox { page, index, with_text: true });
        view.box_context = None;
        ui.close();
    }
}
