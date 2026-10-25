//! Clipboard output.

use std::time::Duration;

use kadra_core::BgraImage;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIBV5, CF_UNICODETEXT};
use windows::core::Result;

/// `LCS_sRGB` colour space tag.
const LCS_SRGB: u32 = u32::from_be_bytes(*b"sRGB");
/// `LCS_GM_IMAGES` rendering intent.
const LCS_GM_IMAGES: u32 = 4;

/// Another application may hold the clipboard for a moment.
fn open(owner: HWND) -> Result<()> {
    let mut attempt = 0;
    loop {
        // SAFETY: `owner` is a window of this thread.
        match unsafe { OpenClipboard(Some(owner)) } {
            Ok(()) => return Ok(()),
            Err(_) if attempt < 10 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => return Err(err),
        }
    }
}

/// Replaces the clipboard content with a single format built by `fill`.
fn set(owner: HWND, format: u32, size: usize, fill: impl FnOnce(&mut [u8])) -> Result<()> {
    open(owner)?;
    // SAFETY: the clipboard is open for this thread; the global memory is
    // either handed to the system or freed.
    let result = unsafe {
        (|| {
            EmptyClipboard()?;
            let memory = GlobalAlloc(GMEM_MOVEABLE, size)?;
            let ptr = GlobalLock(memory).cast::<u8>();
            if ptr.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err(windows::core::Error::from_thread());
            }
            fill(std::slice::from_raw_parts_mut(ptr, size));
            let _ = GlobalUnlock(memory);
            if let Err(err) = SetClipboardData(format, Some(HANDLE(memory.0))) {
                let _ = GlobalFree(Some(memory));
                return Err(err);
            }
            Ok(())
        })()
    };
    // SAFETY: matches the successful `open` above.
    let _ = unsafe { CloseClipboard() };
    result
}

/// Copies an image as `CF_DIBV5` (32-bit with alpha). Windows synthesises
/// `CF_DIB` and `CF_BITMAP` for applications that need them.
pub fn set_image(owner: HWND, image: &BgraImage) -> Result<()> {
    let header_size = size_of::<BITMAPV5HEADER>();
    let stride = image.stride();
    let pixels = stride * image.height() as usize;

    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let header = BITMAPV5HEADER {
        bV5Size: header_size as u32,
        bV5Width: image.width() as i32,
        // Positive height: bottom-up rows, the layout every application reads.
        bV5Height: image.height() as i32,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: pixels as u32,
        bV5RedMask: 0x00FF_0000,
        bV5GreenMask: 0x0000_FF00,
        bV5BlueMask: 0x0000_00FF,
        bV5AlphaMask: 0xFF00_0000,
        bV5CSType: LCS_SRGB,
        bV5Intent: LCS_GM_IMAGES,
        ..Default::default()
    };

    set(owner, u32::from(CF_DIBV5.0), header_size + pixels, |buf| {
        // SAFETY: BITMAPV5HEADER is plain old data and `buf` is large enough.
        let header_bytes = unsafe {
            std::slice::from_raw_parts(std::ptr::from_ref(&header).cast::<u8>(), header_size)
        };
        buf[..header_size].copy_from_slice(header_bytes);
        let src = image.as_bytes();
        for (dst_row, src_row) in buf[header_size..]
            .chunks_exact_mut(stride)
            .zip(src.chunks_exact(stride).rev())
        {
            dst_row.copy_from_slice(src_row);
        }
    })
}

/// Copies plain text.
pub fn set_text(owner: HWND, text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    set(owner, u32::from(CF_UNICODETEXT.0), wide.len() * 2, |buf| {
        for (dst, unit) in buf.as_chunks_mut::<2>().0.iter_mut().zip(&wide) {
            *dst = unit.to_le_bytes();
        }
    })
}
