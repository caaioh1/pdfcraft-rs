use super::*;

fn masked_shading(shading: &str, extra: &str, ink: &str) -> Vec<u8> {
    let content = format!("20.25 20.25 279.5 279.5 re W n /M gs {ink} 0 0 320 320 re f");
    let pdf = format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 320 320] /Contents 4 0 R /Resources << /ExtGState << /M 5 0 R >> /Pattern << /P << /PatternType 2 /Shading << /ShadingType 3 /ColorSpace /DeviceRGB /Coords [160 160 0 160 160 180] /Extend [true true] /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [1 0.5 0] /N 1 >> >> >> >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /Type /ExtGState /SMask << /S /Luminosity /G 6 0 R >> >> endobj
6 0 obj << /Type /XObject /Subtype /Form /BBox [0 0 320 320] /Group << /S /Transparency /CS /DeviceCMYK >> /Resources << /Shading << /Sh0 {shading} >> >> /Length 7 >> stream
/Sh0 sh
endstream endobj
{extra}
trailer << /Root 1 0 R >>
%%EOF",
        content.len()
    );
    pdf.into_bytes()
}

fn render_mask(bytes: Vec<u8>, request: RenderRequest) -> RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let page = renderer.render(request);
    assert!(page.error.is_none(), "{:?}", page.error);
    page
}

#[test]
fn radial_luminosity_mask_fades_when_painted_by_one_shading() {
    let bytes = masked_shading(RADIAL_MASK, "", "/Pattern cs /P scn");

    let page = render_mask(bytes, RenderRequest { scale: 1.0, ..Default::default() });

    for (x, blue) in [(160_u32, 0..=5), (250, 124..=132), (299, 200..=250), (300, 255..=255)] {
        let offset = usize::try_from((160 * page.width + x) * 4).unwrap();
        let pixel = &page.rgba[offset..offset + 4];
        assert!(blue.contains(&pixel[2]), "centre/midpoint/antialiased clip/outside, x {x}: {pixel:?}, blue {blue:?}");
    }
}

const RADIAL_MASK: &str = "<< /ShadingType 3 /ColorSpace /DeviceGray /Coords [160 160 0 160 160 180] /Extend [true true] /AntiAlias false /Function << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >> >>";

#[test]
fn radial_luminosity_mask_matches_per_pixel_shading_when_scaled_or_tiled() {
    let code = "{ 160 sub dup mul exch 160 sub dup mul add sqrt 180 div 1 exch sub }";
    let extra = format!("7 0 obj << /FunctionType 4 /Domain [0 320 0 320] /Range [0 1] /Length {} >> stream\n{code}\nendstream endobj", code.len());
    for ink in ["1 0 0 rg", "/Pattern cs /P scn"] {
        let radial = masked_shading(RADIAL_MASK, "", ink);
        let sampled = masked_shading("<< /ShadingType 1 /ColorSpace /DeviceGray /Domain [0 320 0 320] /Function 7 0 R >>", &extra, ink);
        for scale in [0.5, 1.0, 2.0] {
            for preview in [false, true] {
                for tile in [None, Some(Tile { x: 64, y: 64, w: 160, h: 160 })] {
                    let request = RenderRequest { scale, preview, tile, ..Default::default() };

                    let native = render_mask(radial.clone(), request);
                    let reference = render_mask(sampled.clone(), request);

                    assert_eq!((native.width, native.height), (reference.width, reference.height));
                    let worst = native.rgba.iter().zip(reference.rgba.iter()).map(|(a, b)| a.abs_diff(*b)).max();
                    assert!(worst.is_some_and(|d| d <= 4), "{ink}, {request:?}: radial luminosity mask differs from sampled reference by {worst:?}");
                }
            }
        }
    }
}

/// A 100 × 100 pt page filled by a function-based shading over the unit square (Matrix 100), whose
/// colour comes from the calculator function `code` (inputs x y, outputs r g b).
fn function_shading_page(code: &str) -> Vec<u8> {
    let content = "/Sh0 sh";
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /Shading << /Sh0 << /ShadingType 1 /ColorSpace /DeviceRGB /Domain [0 1 0 1] /Matrix [100 0 0 100 0 0] /Function 5 0 R >> >> >> >> endobj
4 0 obj << /Length {} >> stream
{content}
endstream endobj
5 0 obj << /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1] /Length {} >> stream
{code}
endstream endobj
trailer << /Root 1 0 R >>
%%EOF",
        content.len(),
        code.len()
    )
    .into_bytes()
}

/// The red and green of the pixel at (`x`, `y`) of a 100 × 100 render, y down.
fn red_green(page: &RenderedPage, x: u32, y: u32) -> (u8, u8) {
    let offset = usize::try_from((y * page.width + x) * 4).unwrap();
    (page.rgba[offset], page.rgba[offset + 1])
}

/// Chrome's print-to-PDF gradients ignore one input; the renderer then evaluates the function once
/// per value of the other. That must paint what evaluating it per pixel does: the colour follows
/// x alone, y alone, or both.
#[test]
fn function_shading_that_ignores_one_input_is_still_painted_by_the_other() {
    let request = RenderRequest { scale: 1.0, ..Default::default() };
    // Inputs are x then y; `pop` drops y, `exch pop` drops x.
    let by_x = render_mask(function_shading_page("{ pop dup 0 }"), request);
    let by_y = render_mask(function_shading_page("{ exch pop dup 0 }"), request);
    let by_both = render_mask(function_shading_page("{ 0 }"), request);

    for (a, b) in [(10_u32, 90_u32), (50, 50), (90, 10)] {
        let (r, g) = red_green(&by_x, a, b);
        assert!(r.abs_diff((a * 255 / 100) as u8) <= 6 && r == g, "x only, ({a}, {b}): red {r}, green {g}");
        // The page's y axis points up, so the row `b` from the top is y = 1 - b / 100.
        let (r, g) = red_green(&by_y, a, b);
        assert!(r.abs_diff(((100 - b) * 255 / 100) as u8) <= 6 && r == g, "y only, ({a}, {b}): red {r}, green {g}");
        let (r, g) = red_green(&by_both, a, b);
        assert!(r.abs_diff((a * 255 / 100) as u8) <= 6, "x and y, red at ({a}, {b}): {r}");
        assert!(g.abs_diff(((100 - b) * 255 / 100) as u8) <= 6, "x and y, green at ({a}, {b}): {g}");
    }
}
