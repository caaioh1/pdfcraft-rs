//! Watermarks other programs wrote: content tagged `/Artifact << /Subtype /Watermark >> BDC … EMC`
//! (ISO 32000-2 §14.8.2.2), the way Acrobat, Word and most PDF libraries mark theirs.
//! [`crate::remove_marks`] only knows the streams PdfCraft tagged itself.
//! shortcut: a property list given by resource name (`/Artifact /P0 BDC`) and `/Watermark`
//! annotations are not recognised; add them when a document turns up that needs them.

use crate::EditError;
use pdfcraft_content::Op;
use pdfcraft_cos::Document;
use std::collections::BTreeSet;

/// Indexes of the operators making up each watermark block (`BDC` through its `EMC`).
fn watermark_ops(ops: &[Op]) -> BTreeSet<usize> {
    let mut remove = BTreeSet::new();
    // Where each open marked-content block started, and whether it is a watermark.
    let mut open: Vec<(usize, bool)> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            b"BDC" => {
                let watermark = op.name(0) == Some(&b"Artifact"[..])
                    && op.operands.get(1).and_then(|o| o.as_dict()).is_some_and(|d| d.name(b"Subtype") == Some(&b"Watermark"[..]));
                open.push((i, watermark));
            }
            b"BMC" => open.push((i, false)),
            b"EMC" => {
                if let Some((start, true)) = open.pop() {
                    remove.extend(start..=i);
                }
            }
            _ => {}
        }
    }
    remove
}

/// Remove the watermark blocks from `page` (0-based); returns how many. Nothing changes unless
/// the rewritten content can be written.
pub(crate) fn remove(doc: &mut Document, page: usize) -> Result<usize, EditError> {
    let streams = crate::objects::checked_streams(doc, page)?;
    let pieces = pdfcraft_content::Pieces::join(&streams.iter().map(|s| s.data.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let gone = watermark_ops(&ops);
    let blocks = gone.iter().filter(|&&i| ops.get(i).is_some_and(|o| o.is("BDC"))).count();
    if blocks == 0 {
        return Ok(0);
    }
    let new = crate::text::splice(&pieces, &ops, |i| (Vec::new(), !gone.contains(&i)));
    let mut work = doc.clone();
    let contents = crate::text::rewritten_contents(&mut work, &streams, new)?;
    let p = pdfcraft_model::pages(&work).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))?;
    work.update_dict(p.obj, |d| {
        d.set(b"Contents".to_vec(), contents);
    })?;
    *doc = work;
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_artifact_watermark_blocks_are_found() {
        let ops = pdfcraft_content::parse(
            b"/Artifact << /Subtype /Watermark >> BDC /P << >> BDC BT ET EMC 0 0 1 1 re f EMC /Artifact << /Subtype /Footer >> BDC EMC /Span BMC EMC",
        )
        .ops;
        let gone = watermark_ops(&ops);
        // BDC, nested BDC, BT, ET, EMC, re, f, EMC: the first eight operators; the footer stays.
        assert_eq!(gone.into_iter().collect::<Vec<_>>(), (0..8).collect::<Vec<_>>());
    }
}
