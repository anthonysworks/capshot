//! Videos drawn over the edited one, picture in picture: each is decoded in
//! step with the main video into a texture that the painter draws (see
//! [`crate::paint::LayerSurface`]).

use std::cell::RefCell;

use capshot_capture::Gpu;
use capshot_core::Rect;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Direct3D11::ID3D11VideoProcessorOutputView;
use windows::Win32::Media::MediaFoundation::{
    IMFDXGIDeviceManager, IMFSample, IMFSourceReader, MF_SOURCE_READERF_ENDOFSTREAM,
    MF_SOURCE_READERF_ERROR,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::GUID;

use crate::Result;
use crate::compose::VideoLayer;
use crate::convert::{Blitter, Space, drawable_texture};
use crate::media::{Decoded, VIDEO, VideoFile};
use crate::paint::LayerSurface;

/// Where the playing layer stands.
#[derive(Default)]
struct Position {
    /// Time in the layer's file of the frame on the texture.
    shown: Option<f64>,
    /// A frame read too early to show yet.
    held: Option<(f64, IMFSample)>,
    /// The file has no more frames: the last one stays.
    ended: bool,
}

pub(crate) struct LayerPlayer {
    layer: VideoLayer,
    reader: IMFSourceReader,
    decoded: Decoded,
    /// Decoded NV12 → the texture.
    scaler: Blitter,
    output: ID3D11VideoProcessorOutputView,
    texture: ID3D11Texture2D,
    /// The visible part of the decoded frame, and the whole texture.
    visible: Rect,
    full: Rect,
    position: RefCell<Position>,
}

#[allow(clippy::cast_possible_wrap)]
impl LayerPlayer {
    /// Opens the file of `layer`.
    ///
    /// SAFETY: Media Foundation must be started.
    pub(crate) unsafe fn open(
        gpu: &Gpu,
        manager: &IMFDXGIDeviceManager,
        layer: &VideoLayer,
    ) -> Result<Self> {
        // SAFETY: forwarded.
        let file = unsafe { VideoFile::open(&layer.path, manager)? };
        // NV12 and the video processor want even sizes.
        let even = |v: i32| u32::try_from(v.max(2)).unwrap_or(2) & !1;
        let size = (even(layer.w), even(layer.h));
        let scaler = Blitter::new(
            gpu,
            file.layout.coded,
            size,
            file.fps,
            (file.space, Space::Rgb),
        )?;
        let texture = drawable_texture(gpu, size)?;
        let output = scaler.output(&texture)?;
        Ok(Self {
            layer: layer.clone(),
            reader: file.reader,
            decoded: Decoded::new(gpu, file.layout.coded),
            scaler,
            output,
            texture,
            visible: file.layout.visible,
            full: Rect::new(0, 0, size.0 as i32, size.1 as i32),
            position: RefCell::new(Position::default()),
        })
    }

    /// What the painter draws.
    pub(crate) fn surface(&self) -> LayerSurface {
        LayerSurface {
            texture: self.texture.clone(),
            rect: Rect::from_size(self.layer.x, self.layer.y, self.layer.w, self.layer.h),
            start: self.layer.start,
            end: self.layer.end,
            opacity: self.layer.opacity.clamp(0.0, 1.0),
        }
    }

    /// The next frame of the file and its time in seconds, or `None` at the
    /// end.
    fn next(&self) -> Result<Option<(f64, IMFSample)>> {
        loop {
            let (mut flags, mut time) = (0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            // SAFETY: Media Foundation call with valid out pointers.
            unsafe {
                self.reader.ReadSample(
                    VIDEO,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )?;
            }
            #[allow(clippy::cast_sign_loss)]
            if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 | MF_SOURCE_READERF_ERROR.0) as u32 != 0 {
                return Ok(None);
            }
            if let Some(sample) = sample {
                #[allow(clippy::cast_precision_loss)]
                return Ok(Some((time as f64 / 1e7, sample)));
            }
        }
    }

    fn show(&self, sample: &IMFSample) -> Result<()> {
        let (texture, slice) = self.decoded.texture(sample)?;
        let input = self.scaler.input(&texture, slice)?;
        Ok(self
            .scaler
            .blit(&input, self.visible, &self.output, self.full)?)
    }

    /// Brings the texture to the frame of source time `t` (seconds of the
    /// main video); does nothing outside the layer's time range. Frames are
    /// read in order; a jump (a cut in the main video) seeks.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub(crate) fn update(&self, t: f64) -> Result<()> {
        if t < self.layer.start || t >= self.layer.end {
            return Ok(());
        }
        let want = (t - self.layer.start + self.layer.from).max(0.0);
        let mut position = self.position.borrow_mut();
        let jumped = position
            .shown
            .is_none_or(|shown| want < shown - 0.05 || want > shown + 1.0);
        if jumped {
            // SAFETY: Media Foundation call with a valid position.
            unsafe {
                self.reader
                    .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from((want * 1e7) as i64))?;
            }
            *position = Position::default();
        }
        loop {
            let next = match position.held.take() {
                Some(held) => Some(held),
                None if position.ended => break,
                None => self.next()?,
            };
            let Some((time, sample)) = next else {
                position.ended = true;
                break;
            };
            // Too early for this frame: it is for a later one.
            if time > want && position.shown.is_some() {
                position.held = Some((time, sample));
                break;
            }
            self.show(&sample)?;
            position.shown = Some(time);
        }
        Ok(())
    }
}
