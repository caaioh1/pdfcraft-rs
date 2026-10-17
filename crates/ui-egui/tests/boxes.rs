//! Edit a PDF ▸ Edit text & images: right-click a drawn box (a shaded, framed note) to delete it,
//! with or without the text inside. Synthetic artwork and standard-font text only.
use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// A 400×500 page: a shaded, framed note (80..320 × 300..380) with a line of text inside it, a
/// yellow highlight further down, and a line of text outside every box.
fn fixture() -> Vec<u8> {
    let body = "0.96 0.84 0.5 rg 80 300 m 320 300 l 320 380 l 80 380 l 80 300 l h f*\n\
        0.8 0.6 0 RG q 1 w 80 300 m 320 300 l S Q q 1 w 320 300 m 320 380 l S Q q 1 w 320 380 m 80 380 l S Q q 1 w 80 380 m 80 300 l S Q\n\
        1 1 0 rg 60 150 80 16 re f*\n\
        0 g BT /F1 12 Tf 100 340 Td (Check this with the client) Tj ET\n\
        BT /F1 12 Tf 100 100 Td (Keep this line) Tj ET\n";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 500] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.session = pdfcraft_engine::Session::new().with_clock(|| 1_700_000_000);
        app.set_option("language", "en").unwrap();
        app.open_bytes("note.pdf", None, fixture()).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h
}

/// A point on the page, in points from its top-left corner.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    r.min + egui::vec2(x, y) * (r.width() / 400.0)
}

fn right_click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() });
    }
    h.run_steps(3);
}

fn box_count(h: &Harness<'static, PdfCraftApp>) -> usize {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().drawn_boxes(0).unwrap().len()
}

fn page_text(h: &Harness<'static, PdfCraftApp>) -> String {
    let app = h.state();
    app.session.get(app.views[0].id).unwrap().text_lines(0).iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_right_click_inside_a_note_offers_to_delete_the_box() {
    let mut h = harness();
    assert_eq!(box_count(&h), 2, "the note and the highlight");
    // On the note's text: the box under it is the note.
    let p = at(&h, 160.0, 140.0);
    right_click(&mut h, p);
    h.get_by_label("Delete box");
    h.get_by_label("Delete box and its text");
    h.get_by_label("Delete box").click();
    h.run_steps(4);
    assert_eq!(box_count(&h), 1, "only the highlight is left");
    assert!(page_text(&h).contains("Check this with the client"), "the text stays when only the box goes");
    let app = h.state();
    assert_eq!(app.session.get(app.views[0].id).unwrap().can_undo(), Some("Delete box"));
}

#[test]
fn delete_box_and_its_text_removes_both_and_one_undo_brings_them_back() {
    let mut h = harness();
    let p = at(&h, 120.0, 130.0);
    right_click(&mut h, p);
    h.get_by_label("Delete box and its text").click();
    h.run_steps(4);
    assert_eq!(box_count(&h), 1);
    let text = page_text(&h);
    assert!(!text.contains("Check this with the client"), "{text}");
    assert!(text.contains("Keep this line"), "{text}");
    let id = h.state().views[0].id;
    h.state_mut().session.undo(id).unwrap();
    h.run_steps(3);
    assert_eq!(box_count(&h), 2);
    assert!(page_text(&h).contains("Check this with the client"));
}

#[test]
fn a_right_click_outside_every_box_offers_no_box_menu() {
    let mut h = harness();
    let p = at(&h, 200.0, 400.0);
    right_click(&mut h, p);
    assert!(h.query_by_label("Delete box").is_none());
    assert_eq!(box_count(&h), 2);
}
