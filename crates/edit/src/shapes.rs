//! Boxes: rectangles a page draws with a fill, a border or both (a shaded note, a framed
//! paragraph, a highlight). They are plain path painting in the page content, so Edit a PDF
//! could neither select nor delete them; deleting the text inside left the empty frame behind.
//!
//! A box is found from the painting operators themselves: a filled rectangle, a stroked
//! rectangle, or a filled rectangle whose four edges were also stroked as separate lines (what
//! Word and LibreOffice write for a bordered, shaded paragraph, often twice). Everything that
//! belongs to one box is deleted together, and only those operators are removed: the rest of the
//! content stream is copied byte for byte, so text, images and graphics state are untouched.
//! Curves, rotated shapes, shapes inside Form XObjects and anything with a clipping path are not
//! boxes.

use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{Document, Object};

use crate::EditError;

/// A shape smaller than this (points) in either direction is a rule or underline, not a box.
const MIN_SIDE: f64 = 3.0;
/// How far an edge may sit from its rectangle's side and still belong to the box.
const EDGE_TOLERANCE: f64 = 2.0;
/// A rectangle covering this share of the page is its background, never listed as a box.
const BACKGROUND_SHARE: f64 = 0.95;
const MAX_DEPTH: usize = 1024;
const MAX_PRIMITIVES: usize = 20_000;
const MAX_BOXES: usize = 2_000;

/// A box on a page, in user space. Indexes come from [`drawn_boxes`] and change when the page
/// content changes, so list again before each edit.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnBox {
    /// `[x0, y0, x1, y1]`, including its border.
    pub rect: [f64; 4],
    pub filled: bool,
    pub stroked: bool,
    /// The fill colour when the page sets it with `g`, `rg` or `k`.
    pub fill: Option<[f64; 3]>,
    /// The operators (indexes into the page's joined content) that draw it.
    ops: Vec<usize>,
}

impl DrawnBox {
    /// How many painting operators draw this box (a bordered shaded one has several).
    pub fn operators(&self) -> usize {
        self.ops.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    Rect,
    /// An axis-aligned line, `true` when horizontal.
    Line(bool),
}

#[derive(Clone, Debug)]
struct Primitive {
    shape: Shape,
    rect: [f64; 4],
    filled: bool,
    stroked: bool,
    fill: Option<[f64; 3]>,
    /// First and last operator (inclusive) of the path and its painting operator.
    ops: (usize, usize),
}

fn near(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance
}

fn cmyk_to_rgb(c: f64, m: f64, y: f64, k: f64) -> [f64; 3] {
    [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
}

/// The boxes on `page` (0-based).
pub fn drawn_boxes(doc: &Document, page: usize) -> Result<Vec<DrawnBox>, EditError> {
    let (_, pieces) = page_pieces(doc, page)?;
    let ops = pieces.parse().ops;
    let p = pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    let crop = p.crop(doc);
    let primitives = primitives(&ops)?;
    Ok(group(primitives, crop))
}

/// Joined content of `page` with its streams, refused when it is unreadable or too large.
/// A page's content streams (object, decoded bytes).
type Streams = Vec<(Object, Vec<u8>)>;

fn page_pieces(doc: &Document, page: usize) -> Result<(Streams, Pieces), EditError> {
    let streams = crate::objects::checked_streams(doc, page)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
    Ok((streams, pieces))
}

struct Path {
    first: usize,
    /// Each subpath's points, already in page space.
    subpaths: Vec<Vec<(f64, f64)>>,
    curved: bool,
    clip: bool,
}

fn primitives(ops: &[Op]) -> Result<Vec<Primitive>, EditError> {
    let mut out = Vec::new();
    let mut ctm = Matrix::IDENTITY;
    let mut stack: Vec<(Matrix, Option<[f64; 3]>)> = Vec::new();
    let mut fill: Option<[f64; 3]> = None;
    let mut path: Option<Path> = None;
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"q" => {
                if stack.len() >= MAX_DEPTH {
                    return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
                }
                stack.push((ctm, fill));
            }
            b"Q" => {
                if let Some((m, f)) = stack.pop() {
                    ctm = m;
                    fill = f;
                }
            }
            b"cm" => {
                if let Some(m) = Matrix::from_operands(&op.operands) {
                    ctm = m.then(&ctm);
                }
            }
            b"g" => fill = op.num(0).map(|g| [g, g, g]),
            b"rg" => fill = op.nums::<3>(),
            b"k" => fill = op.nums::<4>().map(|[c, m, y, k]| cmyk_to_rgb(c, m, y, k)),
            b"sc" | b"scn" | b"cs" => fill = None,
            b"m" | b"l" => {
                if let Some([x, y]) = op.nums::<2>() {
                    let current = path.get_or_insert_with(|| Path::new(i));
                    let point = ctm.apply(x, y);
                    if op.is("m") {
                        current.subpaths.push(vec![point]);
                    } else if let Some(sub) = current.subpaths.last_mut() {
                        sub.push(point);
                    }
                }
            }
            b"re" => {
                if let Some([x, y, w, h]) = op.nums::<4>() {
                    let current = path.get_or_insert_with(|| Path::new(i));
                    let corners = [ctm.apply(x, y), ctm.apply(x + w, y), ctm.apply(x + w, y + h), ctm.apply(x, y + h)];
                    current.subpaths.push(corners.to_vec());
                }
            }
            b"h" => {}
            b"c" | b"v" | b"y" => {
                path.get_or_insert_with(|| Path::new(i)).curved = true;
            }
            b"W" | b"W*" => {
                if let Some(current) = path.as_mut() {
                    current.clip = true;
                }
            }
            b"f" | b"F" | b"f*" | b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                if let Some(finished) = path.take() {
                    let (filled, stroked) = match op.op.as_slice() {
                        b"f" | b"F" | b"f*" => (true, false),
                        b"S" | b"s" => (false, true),
                        _ => (true, true),
                    };
                    if let Some(p) = finished.finish(i, filled, stroked, fill) {
                        if out.len() >= MAX_PRIMITIVES {
                            return Err(EditError::Invalid("this page has too many shapes to select boxes".into()));
                        }
                        out.push(p);
                    }
                }
            }
            b"n" => path = None,
            _ => {}
        }
    }
    Ok(out)
}

impl Path {
    fn new(first: usize) -> Self {
        Path { first, subpaths: Vec::new(), curved: false, clip: false }
    }

    fn finish(self, paint: usize, filled: bool, stroked: bool, fill: Option<[f64; 3]>) -> Option<Primitive> {
        if self.curved || self.clip {
            return None;
        }
        let [points] = <[Vec<(f64, f64)>; 1]>::try_from(self.subpaths).ok()?;
        let ops = (self.first, paint);
        let finite = points.iter().all(|(x, y)| x.is_finite() && y.is_finite());
        if !finite {
            return None;
        }
        let bounds = |pts: &[(f64, f64)]| {
            pts.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, (x, y)| [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)])
        };
        let mut distinct = points.clone();
        if distinct.len() == 5 && distinct.first() == distinct.last() {
            distinct.pop();
        }
        if distinct.len() == 4 && is_axis_aligned_rect(&distinct) {
            return Some(Primitive { shape: Shape::Rect, rect: bounds(&distinct), filled, stroked, fill, ops });
        }
        if let [a, b] = points.as_slice()
            && stroked
            && !filled
        {
            let horizontal = near(a.1, b.1, 0.25);
            if horizontal || near(a.0, b.0, 0.25) {
                return Some(Primitive { shape: Shape::Line(horizontal), rect: bounds(&points), filled, stroked, fill, ops });
            }
        }
        None
    }
}

/// Four corners in drawing order forming an axis-aligned rectangle.
fn is_axis_aligned_rect(p: &[(f64, f64)]) -> bool {
    let t = 0.01;
    let at = |i: usize| p.get(i).copied();
    let (Some(a), Some(b), Some(c), Some(d)) = (at(0), at(1), at(2), at(3)) else {
        return false;
    };
    (near(a.1, b.1, t) && near(b.0, c.0, t) && near(c.1, d.1, t) && near(d.0, a.0, t))
        || (near(a.0, b.0, t) && near(b.1, c.1, t) && near(c.0, d.0, t) && near(d.1, a.1, t))
}

fn area(r: [f64; 4]) -> f64 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

fn same_rect(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| near(*x, y, EDGE_TOLERANCE))
}

/// Does the line `edge` lie along a side of `rect`?
fn along_side(edge: &Primitive, rect: [f64; 4]) -> bool {
    let Shape::Line(horizontal) = edge.shape else {
        return false;
    };
    let t = EDGE_TOLERANCE;
    let r = edge.rect;
    if horizontal {
        (near(r[1], rect[1], t) || near(r[1], rect[3], t)) && r[0] >= rect[0] - t && r[2] <= rect[2] + t
    } else {
        (near(r[0], rect[0], t) || near(r[0], rect[2], t)) && r[1] >= rect[1] - t && r[3] <= rect[3] + t
    }
}

fn group(primitives: Vec<Primitive>, page: [f64; 4]) -> Vec<DrawnBox> {
    let page_area = area(page);
    let mut boxes: Vec<DrawnBox> = Vec::new();
    let mut claimed = vec![false; primitives.len()];
    let wide_enough = |r: [f64; 4]| r[2] - r[0] >= MIN_SIDE && r[3] - r[1] >= MIN_SIDE;
    let is_background = |r: [f64; 4]| page_area > 0.0 && area(r) >= page_area * BACKGROUND_SHARE;
    // Largest rectangles first, so a note's frame takes its own edges before a highlight inside it.
    let mut seeds: Vec<usize> = primitives
        .iter()
        .enumerate()
        .filter(|(_, p)| p.shape == Shape::Rect && wide_enough(p.rect) && !is_background(p.rect))
        .map(|(i, _)| i)
        .collect();
    seeds.sort_by(|a, b| {
        let (x, y) = (primitives.get(*a).map_or(0.0, |p| area(p.rect)), primitives.get(*b).map_or(0.0, |p| area(p.rect)));
        y.total_cmp(&x)
    });
    for seed in seeds {
        if boxes.len() >= MAX_BOXES {
            break;
        }
        let Some(first) = primitives.get(seed).filter(|_| !claimed.get(seed).copied().unwrap_or(true)) else {
            continue;
        };
        let rect = first.rect;
        let mut members = Vec::new();
        for (i, p) in primitives.iter().enumerate() {
            if claimed.get(i).copied().unwrap_or(true) {
                continue;
            }
            let part = match p.shape {
                Shape::Rect => same_rect(p.rect, rect),
                Shape::Line(_) => along_side(p, rect),
            };
            if part {
                members.push(i);
            }
        }
        let mut ops = Vec::new();
        let (mut filled, mut stroked, mut fill) = (false, false, None);
        let mut bounds = rect;
        for i in members {
            if let Some(slot) = claimed.get_mut(i) {
                *slot = true;
            }
            let Some(p) = primitives.get(i) else { continue };
            filled |= p.filled;
            stroked |= p.stroked;
            if p.filled && fill.is_none() {
                fill = p.fill;
            }
            bounds = [bounds[0].min(p.rect[0]), bounds[1].min(p.rect[1]), bounds[2].max(p.rect[2]), bounds[3].max(p.rect[3])];
            ops.extend(p.ops.0..=p.ops.1);
        }
        ops.sort_unstable();
        ops.dedup();
        boxes.push(DrawnBox { rect: bounds, filled, stroked, fill, ops });
    }
    // Reading order: top to bottom, then left to right (PDF space has y up).
    boxes.sort_by(|a, b| b.rect[3].total_cmp(&a.rect[3]).then(a.rect[0].total_cmp(&b.rect[0])));
    boxes
}

/// Delete box `index` (from [`drawn_boxes`] on the same page content). With `with_text`, the lines
/// of text that lie wholly inside it go too, so deleting a note leaves neither frame nor words.
/// Returns how many lines of text were removed. Nothing changes unless everything succeeds.
pub fn delete_drawn_box(doc: &mut Document, page: usize, index: usize, with_text: bool) -> Result<usize, EditError> {
    let boxes = drawn_boxes(doc, page)?;
    let target =
        boxes.get(index).ok_or_else(|| EditError::Invalid(format!("page {} has no box {}", page.saturating_add(1), index.saturating_add(1))))?;
    let rect = target.rect;
    let (streams, pieces) = page_pieces(doc, page)?;
    let ops = pieces.parse().ops;
    let remove: std::collections::BTreeSet<usize> = target.ops.iter().copied().collect();
    let new = crate::text::splice(&pieces, &ops, |i| (Vec::new(), !remove.contains(&i)));
    let mut work = doc.clone();
    let contents = crate::text::rewritten_contents(&mut work, &streams, new);
    let p = pdfcraft_model::pages(&work).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    work.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), contents);
    })?;
    // The page must still list the remaining boxes: a bad rewrite would show up here.
    let after = drawn_boxes(&work, page)?;
    if after.len() + 1 != boxes.len() {
        return Err(EditError::Invalid("the box could not be removed cleanly, so nothing was changed".into()));
    }
    let mut removed = 0;
    if with_text {
        // Highest first: removing a line renumbers the ones after it.
        let inside: Vec<usize> = crate::text::text_lines(&work, page)?
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.text.trim().is_empty() && pdfcraft_content::contains(rect, l.rect, 1.0))
            .map(|(i, _)| i)
            .collect();
        for line in inside.into_iter().rev() {
            crate::text::replace_line(&mut work, page, line, "")?;
            removed += 1;
        }
    }
    *doc = work;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{SaveOptions, write_incremental};

    use super::*;

    /// A one-page 600×800 document whose content is `streams`, one stream each.
    fn doc_with(streams: &[&str]) -> Document {
        let n = streams.len();
        let refs: Vec<String> = (0..n).map(|i| format!("{} 0 R", 5 + i)).collect();
        let mut objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>".into(),
            format!("<< /Type /Page /Parent 2 0 R /Contents [{}] /Resources << /Font << /F1 4 0 R >> >> >>", refs.join(" ")),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ];
        for s in streams {
            objs.push(format!("<< /Length {} >>\nstream\n{s}\nendstream", s.len()));
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let x = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    /// What a Word or LibreOffice export draws for a shaded, bordered note: a filled polygon,
    /// then the four edges as separate stroked lines, twice.
    const NOTE: &str = "/Artifact BMC\n1 0.956862745 0.8392156862 rg\n88.55 422.339 m\n535.05 422.339 l 535.05 355.039 l 88.55 355.039 l 88.55 422.339 l\nh\nf*\nEMC\n\
        /Artifact BMC\n0.85 0.64 0 RG\n\
        q 1 w 0 J 1 j\n88.6 421.789 m\n535.05 421.789 l S\nQ\nq 1 w 0 J 1 j\n534.55 422.289 m\n534.55 355.039 l S\nQ\n\
        q 1 w 0 J 1 j\n535.05 355.539 m\n88.6 355.539 l S\nQ\nq 1 w 0 J 1 j\n89.1 355.039 m\n89.1 422.289 l S\nQ\n\
        q 1 w 0 J 1 j\n88.6 421.789 m\n535.05 421.789 l S\nQ\nq 1 w 0 J 1 j\n534.55 422.289 m\n534.55 355.039 l S\nQ\n\
        q 1 w 0 J 1 j\n535.05 355.539 m\n88.6 355.539 l S\nQ\nq 1 w 0 J 1 j\n89.1 355.039 m\n89.1 422.289 l S\nQ\nEMC\n";
    const HIGHLIGHT: &str = "/Artifact BMC\n1 1 0 rg\n106.2 551.139 66.35 14.05 re f*\nEMC\n";
    const TEXT: &str = "BT /F1 9 Tf 100 400 Td (Hello) Tj ET\n";

    fn content(doc: &Document) -> String {
        let p = &pdfcraft_model::pages(doc)[0];
        let list = match doc.resolve(p.dict.get(b"Contents").unwrap()).as_ref() {
            Object::Array(a) => a.clone(),
            _ => vec![p.dict.get(b"Contents").unwrap().clone()],
        };
        list.iter()
            .map(|o| match &*doc.resolve(o) {
                Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_shaded_bordered_note_is_one_box_with_its_edges() {
        let doc = doc_with(&[&format!("{NOTE}{HIGHLIGHT}{TEXT}")]);
        let boxes = drawn_boxes(&doc, 0).unwrap();
        assert_eq!(boxes.len(), 2, "{boxes:?}");
        // Top first: the highlight sits higher on the page than the note.
        let (highlight, note) = (&boxes[0], &boxes[1]);
        assert!(highlight.filled && !highlight.stroked);
        assert_eq!(highlight.fill, Some([1.0, 1.0, 0.0]));
        assert!(note.filled && note.stroked);
        assert!(same_rect(note.rect, [88.55, 355.039, 535.05, 422.339]), "{:?}", note.rect);
        // The fill polygon (7 operators) and eight edge lines (3 each).
        assert_eq!(note.operators(), 7 + 8 * 3);
    }

    #[test]
    fn deleting_a_box_removes_only_its_drawing() {
        let mut doc = doc_with(&[&format!("{NOTE}{HIGHLIGHT}{TEXT}")]);
        let before = content(&doc);
        delete_drawn_box(&mut doc, 0, 1, false).unwrap();
        let after = content(&doc);
        assert!(!after.contains("535.05 422.339 l"), "the fill is gone: {after}");
        assert!(!after.contains("88.6 421.789 m"), "the edges are gone: {after}");
        assert!(after.contains("106.2 551.139 66.35 14.05 re f*"), "the highlight stays");
        assert!(after.contains("(Hello) Tj"), "the text stays");
        assert!(after.matches('q').count() >= after.matches('Q').count(), "graphics state stays balanced");
        assert_eq!(before.matches("\nq 1 w").count(), 8);
        assert_eq!(drawn_boxes(&doc, 0).unwrap().len(), 1);
        // Saving and reading it back keeps the result.
        let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let reopened = Document::open(Arc::new(bytes)).unwrap();
        assert_eq!(drawn_boxes(&reopened, 0).unwrap().len(), 1);
        assert!(content(&reopened).contains("(Hello) Tj"));
    }

    #[test]
    fn a_box_split_across_content_streams_is_still_found_and_deleted() {
        let (a, b) = NOTE.split_at(NOTE.find("EMC\n/Artifact BMC\n0.85").unwrap());
        let mut doc = doc_with(&[a, &format!("{b}{TEXT}")]);
        assert_eq!(drawn_boxes(&doc, 0).unwrap().len(), 1);
        delete_drawn_box(&mut doc, 0, 0, false).unwrap();
        assert!(drawn_boxes(&doc, 0).unwrap().is_empty());
        assert!(content(&doc).contains("(Hello) Tj"));
    }

    #[test]
    fn only_real_boxes_are_listed() {
        // A page-sized background, a thin rule, a curved shape and a clipping rectangle.
        let page = "0 0 600 800 re f\n100 100 300 1 re f\n10 10 m 50 10 80 40 90 90 c f\n50 50 100 100 re W n\n";
        let doc = doc_with(&[page]);
        assert!(drawn_boxes(&doc, 0).unwrap().is_empty());
        // A stroked rectangle is a box on its own.
        let frame = doc_with(&["0 0 1 RG 100 100 200 80 re S\n"]);
        let boxes = drawn_boxes(&frame, 0).unwrap();
        assert_eq!((boxes.len(), boxes[0].filled, boxes[0].stroked), (1, false, true));
    }

    #[test]
    fn transforms_move_the_box_to_where_it_is_drawn() {
        let doc = doc_with(&["q 1 0 0 1 200 300 cm 0 0 50 40 re f Q\n"]);
        let boxes = drawn_boxes(&doc, 0).unwrap();
        assert_eq!(boxes.len(), 1);
        assert!(same_rect(boxes[0].rect, [200.0, 300.0, 250.0, 340.0]), "{:?}", boxes[0].rect);
    }

    #[test]
    fn a_missing_box_changes_nothing() {
        let mut doc = doc_with(&[&format!("{NOTE}{TEXT}")]);
        let before = content(&doc);
        assert!(delete_drawn_box(&mut doc, 0, 5, false).is_err());
        assert!(delete_drawn_box(&mut doc, 3, 0, false).is_err());
        assert_eq!(content(&doc), before);
    }

    #[test]
    fn hostile_content_does_not_panic() {
        let deep = "q ".repeat(5000);
        assert!(drawn_boxes(&doc_with(&[&deep]), 0).is_err());
        for junk in ["re f", "1 2 3 re f", "NaN NaN NaN NaN re f", "1e999 0 5 5 re f", "m l S", "0 0 m S f* B"] {
            let _ = drawn_boxes(&doc_with(&[junk]), 0);
        }
    }
}
