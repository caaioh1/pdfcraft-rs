use super::DxgiPrintContext;
use super::DxgiPrintContextError;
use crate::printer::FilePrinter;
use crate::printer::PrinterDevice;
use crate::ticket::PrintTicket;
use std::path::Path;
use thiserror::Error;
use windows::core::HSTRING;
use windows::Data::Pdf::PdfDocument;
use windows::Storage::StorageFile;
use windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_F;
use windows::Win32::System::WinRT::Pdf::PdfCreateRenderer;
use windows::Win32::System::WinRT::Pdf::PDF_RENDER_PARAMS;

/// PdfCraft patch: the resolution, in dots per inch, pages are drawn at when printing. 300 keeps
/// scans and photographs sharp without the cost of drawing at the printer's top resolution.
/// `WINPRINT_PDF_DPI` (96 to 1200) overrides it.
const PDF_RENDER_DPI: f32 = 300.0;
/// PdfCraft patch: no page is drawn larger than this many pixels on its longest side, so a poster
/// page is drawn at a lower resolution instead of exhausting video memory.
const MAX_RENDER_SIDE_PX: f32 = 8192.0;

/// The resolution to draw a page of `width` x `height` DIPs (1/96 inch) at.
fn render_dpi(width: f32, height: f32) -> f32 {
    let wanted = std::env::var("WINPRINT_PDF_DPI")
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|d| d.is_finite() && (96.0..=1200.0).contains(d))
        .unwrap_or(PDF_RENDER_DPI);
    // `f32::max` ignores a NaN operand, so both sides are checked before they are compared.
    if !width.is_finite() || !height.is_finite() {
        return 96.0;
    }
    let longest_inches = width.max(height) / 96.0;
    if longest_inches <= 0.0 {
        return 96.0;
    }
    wanted.min(MAX_RENDER_SIDE_PX / longest_inches).max(96.0)
}

/// The size in pixels of a page side of `dips` DIPs drawn at `dpi`.
fn destination_pixels(dips: f32, dpi: f32) -> u32 {
    (dips / 96.0 * dpi).round().clamp(1.0, MAX_RENDER_SIDE_PX) as u32
}

#[derive(Error, Debug)]
/// Represents an error from [`WinPdfPrinter`].
pub enum WinPdfPrinterError {
    /// DXGI print context error.
    #[error("DXGI print context error")]
    DxgiPrintContextError(#[from] DxgiPrintContextError),
    /// Invalid path.
    #[error("Invalid path")]
    InvalidPath(#[source] std::io::Error),
    /// Failed to open the document.
    #[error("Failed to open the document")]
    FailedToOpenDocument(#[source] windows::core::Error),
    /// Render error.
    #[error("Render error")]
    RenderError(#[source] windows::core::Error),
}

/// A printer that uses [`Windows.Data.Pdf`](https://learn.microsoft.com/en-us/uwp/api/windows.data.pdf?view=winrt-26100) to to print PDF documents.
pub struct WinPdfPrinter {
    printer: PrinterDevice,
}

impl WinPdfPrinter {
    /// Create a new [`WinPdfPrinter`] for the given printer device.
    pub fn new(printer: PrinterDevice) -> Self {
        Self { printer }
    }
}

impl FilePrinter for WinPdfPrinter {
    type Options = PrintTicket;
    type Error = WinPdfPrinterError;
    fn print(&self, path: &Path, options: PrintTicket) -> Result<(), WinPdfPrinterError> {
        self.print_with_progress(path, options, |_, _| {})
    }
}

impl WinPdfPrinter {
    /// Print one PDF as one job, reporting each page after Direct2D accepts it.
    ///
    /// Counts describe pages processed, not physical output or terminal spooler success.
    /// The final result still reports cancellation/failure even after every page was accepted.
    /// The callback runs synchronously on the calling thread; it does not require `Send`.
    pub fn print_with_progress(
        &self,
        path: &Path,
        options: PrintTicket,
        mut progress: impl FnMut(u32, u32),
    ) -> Result<(), WinPdfPrinterError> {
        let context = DxgiPrintContext::new(
            &self.printer,
            &options,
            path.file_name().unwrap_or(path.as_ref()),
        )?;
        let dxgi_device = &context.dxgi_device;
        let print_control = &context.print_control;
        let d2d_context = &context.d2d_context;
        unsafe {
            let absolute_path =
                std::path::absolute(path).map_err(WinPdfPrinterError::InvalidPath)?;
            let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(absolute_path.as_path()))
                .map_err(WinPdfPrinterError::FailedToOpenDocument)?
                .join()
                .map_err(WinPdfPrinterError::FailedToOpenDocument)?;
            let pdf_document = PdfDocument::LoadFromFileAsync(&file)
                .map_err(WinPdfPrinterError::FailedToOpenDocument)?
                .join()
                .map_err(WinPdfPrinterError::FailedToOpenDocument)?;

            let pdf_renderer =
                PdfCreateRenderer(dxgi_device).map_err(WinPdfPrinterError::RenderError)?;
            let page_count = pdf_document
                .PageCount()
                .map_err(WinPdfPrinterError::RenderError)?;
            progress(0, page_count);
            for i in 0..page_count {
                let pdf_page = pdf_document
                    .GetPage(i)
                    .map_err(WinPdfPrinterError::RenderError)?;

                let pdf_size = pdf_page.Size().map_err(WinPdfPrinterError::RenderError)?;
                // PdfCraft patch: draw the page at print resolution. With a zero destination the
                // renderer sizes its bitmaps for a 96 dpi screen, so a 400 dpi scan was printed at
                // 96 ppi ("a bad scan, out of focus"). The context's dpi and the destination size
                // (in pixels) are raised together; the page keeps its size in DIPs.
                let dpi = render_dpi(pdf_size.Width as f32, pdf_size.Height as f32);
                d2d_context.SetDpi(dpi, dpi);
                let pdf_render_options = PDF_RENDER_PARAMS {
                    IgnoreHighContrast: true.into(),
                    DestinationWidth: destination_pixels(pdf_size.Width as f32, dpi),
                    DestinationHeight: destination_pixels(pdf_size.Height as f32, dpi),
                    ..Default::default()
                };
                let command_list = d2d_context
                    .CreateCommandList()
                    .map_err(WinPdfPrinterError::RenderError)?;
                d2d_context.SetTarget(&command_list);
                // The destination is `dpi / 96` times the page in DIPs, but the print control
                // reads the command list at 96 dpi: scale it back so the page keeps its size.
                let back = 96.0 / dpi;
                d2d_context.SetTransform(&windows_numerics::Matrix3x2 {
                    M11: back,
                    M12: 0.0,
                    M21: 0.0,
                    M22: back,
                    M31: 0.0,
                    M32: 0.0,
                });
                d2d_context.BeginDraw();
                pdf_renderer
                    .RenderPageToDeviceContext(&pdf_page, d2d_context, Some(&pdf_render_options))
                    .map_err(WinPdfPrinterError::RenderError)?;
                d2d_context
                    .EndDraw(None, None)
                    .map_err(WinPdfPrinterError::RenderError)?;
                command_list
                    .Close()
                    .map_err(WinPdfPrinterError::RenderError)?;

                let dx_size = D2D_SIZE_F {
                    width: pdf_size.Width as f32,
                    height: pdf_size.Height as f32,
                };
                print_control
                    .AddPage(&command_list, dx_size, None, None, None)
                    .map_err(WinPdfPrinterError::RenderError)?;
                progress(i + 1, page_count);
            }
        }
        context.close_and_wait()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{destination_pixels, render_dpi};
    use super::WinPdfPrinter;

    /// PdfCraft patch tests: an A4 page (794 x 1123 DIPs) is drawn at 300 dpi, i.e. 2481 x 3509
    /// pixels (not the 794 x 1123 of a screen), a poster is held to the pixel cap, and nonsense
    /// sizes fall back to the screen resolution.
    #[test]
    fn pages_are_drawn_at_print_resolution() {
        let (w, h) = (794.0, 1123.0);
        let dpi = render_dpi(w, h);
        assert_eq!(dpi, 300.0);
        assert_eq!((destination_pixels(w, dpi), destination_pixels(h, dpi)), (2481, 3509));
        // A 4 m poster side would need 47,000 px at 300 dpi: it is held to the cap instead.
        let side = 4.0 / 0.0254 * 96.0;
        let poster = render_dpi(side, side / 2.0);
        assert!(poster < 300.0 && poster >= 96.0, "{poster}");
        assert!(destination_pixels(side, poster) <= 8192);
        for bad in [(0.0, 0.0), (f32::NAN, 10.0), (f32::INFINITY, 10.0), (-5.0, -5.0)] {
            assert_eq!(render_dpi(bad.0, bad.1), 96.0, "{bad:?}");
        }
        assert_eq!(destination_pixels(0.0, 300.0), 1);
    }

    use crate::{printer::FilePrinter, test_utils::null_device};
    use std::path::Path;

    #[test]
    fn print_simple_pdf_document() {
        let device = null_device::thread_local();
        let pdf = WinPdfPrinter::new(device);
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test_data/test_document.pdf");
        pdf.print(path.as_path(), Default::default()).unwrap();
    }
}
