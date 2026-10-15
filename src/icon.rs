/// The lclip mark, drawn at any size: a clipboard with two lines of text.
/// Returns RGBA. `light` is for dark panels (the tray), otherwise ink on paper.
pub fn rgba(size: u32, light: bool) -> Vec<u8> {
    let (fg, bg): ([f32; 3], Option<[f32; 3]>) = if light {
        ([240.0, 239.0, 234.0], None)
    } else {
        ([24.0, 33.0, 42.0], Some([240.0, 239.0, 234.0]))
    };
    let accent = [127.0, 154.0, 123.0];

    let n = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = ((x as f32 + 0.5) / n, (y as f32 + 0.5) / n);

            let board = rrect(px, py, 0.5, 0.56, 0.30, 0.36, 0.08);
            let ring = cover(((-board - 0.055 / 2.0).abs() - 0.055 / 2.0) * n);
            let clip = cover(rrect(px, py, 0.5, 0.17, 0.14, 0.075, 0.035) * n);
            let l1 = cover(rrect(px, py, 0.5, 0.50, 0.15, 0.028, 0.028) * n);
            let l2 = cover(rrect(px, py, 0.46, 0.64, 0.11, 0.028, 0.028) * n);

            let ink = ring.max(clip).max(l1);
            let mut c = [0.0f32; 3];
            let mut a = 0.0f32;
            if let Some(b) = bg {
                // Rounded square background for the window icon.
                let sq = cover(rrect(px, py, 0.5, 0.5, 0.5, 0.5, 0.2) * n);
                c = b;
                a = sq;
            }
            let mix = |c: [f32; 3], a: f32, col: [f32; 3], cov: f32| -> ([f32; 3], f32) {
                let na = cov + a * (1.0 - cov);
                if na <= 0.0 {
                    return (c, 0.0);
                }
                let mut r = [0.0; 3];
                for i in 0..3 {
                    r[i] = (col[i] * cov + c[i] * a * (1.0 - cov)) / na;
                }
                (r, na)
            };
            (c, a) = mix(c, a, fg, ink);
            (c, a) = mix(c, a, accent, l2);

            let i = ((y * size + x) * 4) as usize;
            out[i] = c[0] as u8;
            out[i + 1] = c[1] as u8;
            out[i + 2] = c[2] as u8;
            out[i + 3] = (a * 255.0) as u8;
        }
    }
    out
}

/// StatusNotifierItem wants ARGB32 in network byte order.
pub fn argb(size: u32, light: bool) -> Vec<u8> {
    let mut d = rgba(size, light);
    for p in d.chunks_exact_mut(4) {
        p.rotate_right(1);
    }
    d
}

fn rrect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (dx, dy) = ((px - cx).abs() - (hw - r), (py - cy).abs() - (hh - r));
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - r
}

/// Signed distance in pixels to coverage.
fn cover(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}
