//! Edit a PDF ▸ Edit text & images: the boxes a page draws (a shaded note, a framed paragraph, a
//! highlight). Right-click one for **Delete box** or **Delete box and its text**. The page's
//! own paragraph and picture handling is unchanged: a right-click that lands inside a box opens
//! this menu instead of the picture's, and nothing else about selecting or typing differs.

use craft::ui::contract::{Props as _, Show as _};
use craft::ui::menu::MenuItem;
use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_engine::{DrawnBox, Edit};
use pdfcraft_render::DocInfo;

use crate::canvas::{PageGeometry, PageSession, PageXform};

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
fn box_at(xf: &PageXform, info: &DocInfo, page: usize, boxes: &[DrawnBox], p: Pos2) -> Option<(usize, Rect)> {
    screen_boxes(xf, info, page, boxes)
        .into_iter()
        .filter(|(_, r)| r.expand(1.0).contains(p))
        .min_by(|(_, a), (_, b)| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
}

/// The boxes `page` draws, read once per document generation.
fn page_boxes(session: &mut PageSession, doc: &pdfcraft_engine::Document, page: usize) -> Vec<DrawnBox> {
    let generation = doc.edit_generation();
    match session.edit_boxes.get(&page) {
        Some((g, b)) if *g == generation => b.clone(),
        _ => {
            let b = doc.drawn_boxes(page).unwrap_or_default();
            session.edit_boxes.insert(page, (generation, b.clone()));
            b
        }
    }
}

/// A faint outline around the box the pointer is over on `page`, so a right-click knows what it
/// will take.
pub(crate) fn paint_hover(
    ui: &egui::Ui,
    session: &mut PageSession,
    doc: &pdfcraft_engine::Document,
    info: &DocInfo,
    page: usize,
    geo: PageGeometry<'_>,
) {
    let Some(xf) = session.page_xform(page, geo) else { return };
    let Some(p) = ui.input(|i| i.pointer.hover_pos()).filter(|p| xf.rect.contains(*p)) else { return };
    let boxes = page_boxes(session, doc, page);
    if let Some((_, r)) = box_at(&xf, info, page, &boxes, p) {
        ui.painter().rect_stroke(r, CornerRadius::same(1), Stroke::new(1.0, ACCENT.gamma_multiply(0.55)), egui::StrokeKind::Outside);
    }
}

/// A right-click at `at` on `page` (one the page router left to the page: not on a comment, link
/// or field): inside a box it opens the box menu, and unselects the picture so the menu is the box's.
pub(crate) fn right_click(session: &mut PageSession, doc: &pdfcraft_engine::Document, info: &DocInfo, page: usize, geo: PageGeometry<'_>, at: Pos2) {
    let Some(xf) = session.page_xform(page, geo) else { return };
    let boxes = page_boxes(session, doc, page);
    if let Some((index, _)) = box_at(&xf, info, page, &boxes, at) {
        session.box_context = Some((page, index));
        crate::edit_text_ui::picture_context(page, None, session);
    }
}

/// The menu of box `index` on `page`.
pub(crate) fn menu(ui: &mut egui::Ui, page: usize, index: usize, can_modify: bool, session: &mut PageSession) {
    if MenuItem::new(tl!("Delete box").to_string()).enabled(can_modify).show(ui).ran {
        session.pending_edit = Some(Edit::DeleteDrawnBox { page, index, with_text: false });
        session.box_context = None;
    }
    if MenuItem::new(tl!("Delete box and its text").to_string()).enabled(can_modify).show(ui).ran {
        session.pending_edit = Some(Edit::DeleteDrawnBox { page, index, with_text: true });
        session.box_context = None;
    }
}
