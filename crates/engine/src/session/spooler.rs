//! What the system's print spooler can be given. Windows draws a job with `Windows.Data.Pdf`
//! into Direct2D, and that renderer crashes the whole process (an access violation in
//! `d2d1.dll`, which no `Result` can catch) on a page that draws with a Type 3 font, as Chromium
//! and some court systems write them. Such a job goes as pictures instead, which print everywhere.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use super::{DocId, EditError, Session, print};

/// The resolution a job is drawn at when its pages are sent as pictures to dodge the crash.
const TYPE3_FALLBACK_DPI: f64 = 300.0;
/// How many levels of form XObjects are searched for the fonts a page draws with.
const MAX_FORM_DEPTH: usize = 6;
/// The most dictionaries searched on one page, so a hostile resource tree ends the search.
const MAX_VISITED: usize = 4096;

impl Session {
    /// The print-ready PDF for a job that goes to the system's print spooler (not one that is
    /// saved): [`Session::print_pdf`], with the pages drawn as pictures where the platform's
    /// renderer cannot draw them (a page with a Type 3 font on Windows).
    pub fn print_pdf_for_spooler(&self, id: DocId, settings: &print::Settings) -> Result<Vec<u8>, EditError> {
        if cfg!(windows) && settings.raster.is_none() && self.pages_use_type3(id, &settings.pages) {
            let raster = print::Raster { dpi: TYPE3_FALLBACK_DPI, grayscale: false };
            return self.print_pdf(id, &print::Settings { raster: Some(raster), ..settings.clone() });
        }
        self.print_pdf(id, settings)
    }

    /// Whether any of `pages` (0-based) draws with a Type 3 font. `false` when the document
    /// can't be read: the print itself then reports why.
    fn pages_use_type3(&self, id: DocId, pages: &[usize]) -> bool {
        let Some(doc) = self.get(id) else { return false };
        let cos = match doc.editor.as_ref() {
            Some(e) => e.cos.clone(),
            None => match Document::open_with_password(doc.bytes.clone(), doc.password.as_deref()) {
                Ok(c) => c,
                Err(_) => return false,
            },
        };
        uses_type3(&cos, pages)
    }
}

/// Whether any of `pages` (0-based), or a form they draw, has a Type 3 font in its resources.
pub(crate) fn uses_type3(doc: &Document, pages: &[usize]) -> bool {
    let leaves = pdfcraft_model::pages(doc);
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut visited = 0usize;
    let mut done: HashSet<usize> = HashSet::new();
    for &index in pages {
        if !done.insert(index) {
            continue;
        }
        let Some(page) = leaves.get(index) else { continue };
        let Some(resources) = page.dict.get(b"Resources") else { continue };
        if resources_use_type3(doc, resources, MAX_FORM_DEPTH, &mut seen, &mut visited) {
            return true;
        }
    }
    false
}

fn resources_use_type3(doc: &Document, resources: &Object, depth: usize, seen: &mut HashSet<ObjRef>, visited: &mut usize) -> bool {
    if let Some(r) = resources.as_ref()
        && !seen.insert(r)
    {
        return false;
    }
    *visited = visited.saturating_add(1);
    if *visited > MAX_VISITED {
        return false;
    }
    let resolved = doc.resolve(resources);
    let Some(res) = resolved.as_dict() else { return false };
    if let Some(fonts) = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned())
        && fonts.iter().any(|(_, font)| is_type3(doc, font))
    {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let Some(xobjects) = res.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()) else { return false };
    xobjects.iter().any(|(_, x)| {
        let form = doc.resolve(x);
        let Object::Stream(stream) = &*form else { return false };
        stream.dict.name(b"Subtype") == Some(b"Form")
            && stream.dict.get(b"Resources").is_some_and(|r| resources_use_type3(doc, r, depth - 1, seen, visited))
    })
}

fn is_type3(doc: &Document, font: &Object) -> bool {
    let font = doc.resolve(font);
    let dict: Option<&Dict> = font.as_dict();
    dict.is_some_and(|d| d.name(b"Subtype") == Some(b"Type3"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A one-page PDF (no cross-reference table: the reader rebuilds it) whose page resources are `resources`.
    fn pdf(resources: &str, extra: &str) -> Document {
        let text = format!(
            "%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
             2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
             3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources {resources} >>\nendobj\n{extra}\
             trailer\n<< /Root 1 0 R /Size 20 >>\n%%EOF\n"
        );
        Document::open(Arc::new(text.into_bytes())).expect("the synthetic PDF opens")
    }

    const TYPE3: &str = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << >> /Encoding << >> >>";

    /// Chromium's pages print through a Type 3 font on Windows only as pictures: a page that
    /// names one is found, a page of ordinary fonts is not.
    #[test]
    fn a_type3_font_on_the_page_is_found() {
        let plain = pdf("<< /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >>", "");
        assert!(!uses_type3(&plain, &[0]));
        let direct = pdf(&format!("<< /Font << /F1 {TYPE3} >> >>"), "");
        assert!(uses_type3(&direct, &[0]));
        assert!(!uses_type3(&direct, &[5]), "a page that isn't printed doesn't count");
        let by_ref = pdf("<< /Font << /F1 9 0 R >> >>", &format!("9 0 obj\n{TYPE3}\nendobj\n"));
        assert!(uses_type3(&by_ref, &[0]));
    }

    /// A font drawn by a form on the page counts, and a form that contains itself ends the search.
    #[test]
    fn forms_are_searched_and_cycles_end() {
        let form = |font: &str| {
            format!(
                "9 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /Font << /F1 {font} >> /XObject << /Fm 9 0 R >> >> /Length 0 >>\nstream\n\nendstream\nendobj\n"
            )
        };
        let nested = pdf("<< /XObject << /Fm 9 0 R >> >>", &form(TYPE3));
        assert!(uses_type3(&nested, &[0]));
        let looping = pdf("<< /XObject << /Fm 9 0 R >> >>", &form("<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>"));
        assert!(!uses_type3(&looping, &[0]));
    }
}
