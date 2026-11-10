//! GDI fallback: slower and blind to some hardware overlays, but works
//! everywhere.

use capshot_core::{BgraImage, Point, Rect};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
};

use crate::Result;

pub fn capture(area: Rect, image: &mut BgraImage) -> Result<()> {
    let (w, h) = (area.width(), area.height());
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            #[allow(clippy::cast_possible_truncation)]
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down rows
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    // SAFETY: GDI objects are created, used and released in this block; the
    // DIB bits pointer is valid while the bitmap exists.
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0);

        let result = bitmap.map(|bitmap| {
            let previous = SelectObject(memory, HGDIOBJ(bitmap.0));
            let copied = BitBlt(
                memory,
                0,
                0,
                w,
                h,
                Some(screen),
                area.left,
                area.top,
                SRCCOPY | CAPTUREBLT,
            );
            if copied.is_ok() {
                #[allow(clippy::cast_sign_loss)]
                let (uw, uh) = (w as u32, h as u32);
                let len = uw as usize * uh as usize * 4;
                let data = std::slice::from_raw_parts_mut(bits.cast::<u8>(), len);
                // GDI leaves the alpha channel undefined.
                for px in data.as_chunks_mut::<4>().0 {
                    px[3] = 255;
                }
                image.blit(data, uw as usize * 4, uw, uh, Point::new(0, 0));
            }
            SelectObject(memory, previous);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            copied
        });

        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        result??;
    }
    Ok(())
}
