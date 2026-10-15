//! Debug builds only: sample history, for checking the layout.

use std::{io::Cursor, sync::Arc};

use crate::store::{Format, SECRET_HINT, Sel, Shared};

fn fmt(mime: &str, data: &[u8]) -> Format {
    Format { mime: mime.into(), data: Arc::new(data.to_vec()) }
}

pub fn seed(shared: &Shared) {
    let mut png = Vec::new();
    let img = image::RgbaImage::from_fn(240, 140, |x, y| {
        image::Rgba([(x as f32 / 240.0 * 160.0) as u8 + 40, (y as f32 / 140.0 * 120.0) as u8 + 80, 150, 255])
    });
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();

    let mut s = shared.lock().unwrap();
    s.settings.persist = false;
    s.push(Sel::Primary, vec![fmt("text/plain", b"selected with the mouse")], false);
    s.push(Sel::Primary, vec![fmt("text/plain", b"wlr-data-control")], false);
    s.push(Sel::Clipboard, vec![fmt("text/plain", b"wlr-data-control")], false);
    s.push(
        Sel::Clipboard,
        vec![
            fmt("text/html", b"<h1>Gye Payments design brief</h1><p>Direction A, light.</p>"),
            fmt("text/plain", b"Gye Payments design brief. Direction A, light, with the Somu Labs palette and one accent."),
        ],
        false,
    );
    s.push(Sel::Clipboard, vec![fmt("text/uri-list", b"file:///a/main.rs\nfile:///a/ui.rs\nfile:///a/x11.rs")], false);
    s.push(Sel::Clipboard, vec![fmt("image/png", &png)], false);
    s.push(Sel::Clipboard, vec![fmt("text/plain", b"https://github.com/blackprince001/skills")], false);
    s.push(Sel::Clipboard, vec![fmt("text/plain", b"cargo build --release")], false);
    s.push(Sel::Clipboard, vec![fmt("text/plain", b"hunter2"), fmt(SECRET_HINT, b"secret")], true);
}
