//! Renders typical overlay states over a real capture of the desktop and
//! saves them as PNG files, to review the overlay without clicking around.
//!
//! ```text
//! cargo run -p kadra-overlay --example scenes -- <output-dir>
//! ```

use std::path::PathBuf;

use kadra_capture::{Capturer, Gpu, Snapshot};
use kadra_core::{ImageFormat, Point, RecordingFormat};
use kadra_overlay::layout::{Button, Magnifier, Toolbar};
use kadra_overlay::render::{Renderer, Scene};
use kadra_overlay::selection::{Mode, Purpose, Selection};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

fn scene<'a>(
    snap: &'a Snapshot,
    selection: &'a Selection,
    toolbar: &'a Toolbar,
    hover: Option<Button>,
    purpose: Purpose,
    format: RecordingFormat,
) -> Scene<'a> {
    let cursor = selection.cursor();
    let monitor = snap.monitor_at(cursor);
    Scene {
        selection,
        hover: selection.hover_rect(snap),
        toolbar: (!selection.is_active()).then_some(toolbar),
        toolbar_hover: hover,
        magnifier: Some(Magnifier::new(cursor, monitor.rect, monitor.scale())),
        pixel: snap.image.pixel(cursor),
        scale: monitor.scale(),
        bounds: snap.bounds(),
        purpose,
        format,
        fps: 60,
        countdown: 3,
        system_audio: true,
        microphone: false,
        // `LIGHT=1` renders the light theme.
        light: std::env::var_os("LIGHT").is_some(),
    }
}

const MP4: RecordingFormat = RecordingFormat::Mp4;

// A flat list of scenes: long but simple.
#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    // SAFETY: called once at start-up.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;
        RoInitialize(RO_INIT_MULTITHREADED)?;
    }
    let gpu = Gpu::new()?;
    let renderer = Renderer::new(&gpu)?;
    let snap = Capturer::new(gpu).snapshot(None)?;
    let shot = renderer.load_screenshot(&snap.image)?;

    let monitor = snap
        .monitors
        .iter()
        .find(|m| m.primary)
        .unwrap_or(&snap.monitors[0]);
    let m = monitor.rect;
    let toolbar = Toolbar::new(
        m,
        monitor.scale(),
        Purpose::Screenshot,
        RecordingFormat::Mp4,
    );
    let record_bar = Toolbar::new(m, monitor.scale(), Purpose::Recording, RecordingFormat::Mp4);
    let still = Purpose::Screenshot;
    let at = |fx: f32, fy: f32| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        Point::new(
            m.left + (m.width() as f32 * fx) as i32,
            m.top + (m.height() as f32 * fy) as i32,
        )
    };

    let save = |name: &str, scene: &Scene<'_>| -> Result<(), Box<dyn std::error::Error>> {
        let frame = renderer.render_to_image(scene, &shot)?.crop(m);
        let path = out.join(format!("{name}.png"));
        std::fs::write(&path, kadra_encode::encode(&frame, ImageFormat::Png)?)?;
        println!("{}", path.display());
        Ok(())
    };

    // 1. Hovering a window, toolbar button hovered.
    let hover = Selection::new(Mode::Region, at(0.5, 0.5));
    save(
        "1-hover",
        &scene(
            &snap,
            &hover,
            &toolbar,
            Some(Button::Mode(Mode::Freeform)),
            still,
            MP4,
        ),
    )?;

    // 2. Dragging a rectangle.
    let mut drag = Selection::new(Mode::Region, at(0.2, 0.25));
    drag.press(at(0.2, 0.25), &snap);
    drag.moved(at(0.55, 0.6), &snap);
    save("2-drag", &scene(&snap, &drag, &toolbar, None, still, MP4))?;

    // 3. Drawing a freeform shape (a rough circle).
    let mut free = Selection::new(Mode::Freeform, at(0.5, 0.3));
    free.press(at(0.5, 0.3), &snap);
    for i in 0..=48 {
        let t = f32::from(u8::try_from(i).unwrap_or(0)) / 48.0 * std::f32::consts::TAU * 0.9;
        free.moved(at(0.5 + 0.15 * t.sin(), 0.5 - 0.2 * t.cos()), &snap);
    }
    save(
        "3-freeform",
        &scene(&snap, &free, &toolbar, None, still, MP4),
    )?;

    // 4. Screen mode.
    let screen = Selection::new(Mode::Screen, at(0.7, 0.7));
    save(
        "4-screen",
        &scene(&snap, &screen, &toolbar, Some(Button::Close), still, MP4),
    )?;

    // 5. Magnifier close to the bottom-right corner.
    let corner = Selection::new(Mode::Region, Point::new(m.right - 5, m.bottom - 5));
    save(
        "5-corner",
        &scene(&snap, &corner, &toolbar, None, still, MP4),
    )?;

    // 6. Colour picker.
    let picker = Selection::new(Mode::Picker, at(0.4, 0.4));
    save(
        "6-picker",
        &scene(&snap, &picker, &toolbar, None, still, MP4),
    )?;

    // 7. Recording: rectangular modes and audio toggles (microphone off).
    let record = Selection::new(Mode::Region, at(0.5, 0.5));
    save(
        "7-record",
        &scene(
            &snap,
            &record,
            &record_bar,
            Some(Button::Purpose(Purpose::Screenshot)),
            Purpose::Recording,
            MP4,
        ),
    )?;

    // 8. Recording a GIF: no audio toggles.
    let gif_bar = Toolbar::new(m, monitor.scale(), Purpose::Recording, RecordingFormat::Gif);
    save(
        "8-gif",
        &scene(
            &snap,
            &record,
            &gif_bar,
            None,
            Purpose::Recording,
            RecordingFormat::Gif,
        ),
    )?;

    Ok(())
}
