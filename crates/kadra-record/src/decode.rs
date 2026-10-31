//! Video file decoding with Media Foundation, and conversion to GIF.
//!
//! The source reader decodes (in hardware when the system offers it) and
//! converts frames to 32-bit RGB; frames are then sampled at the GIF frame
//! rate and streamed to [`kadra_gif::GifWriter`], which merges identical
//! frames and stores only what changed.

use std::path::Path;
use std::time::Duration;

use kadra_core::{BgraImage, Rect};
use kadra_gif::{GifOptions, GifSummary, GifWriter};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFAttributes, IMFMediaType, IMFSample, IMFSourceReader, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_SUBTYPE, MF_PD_DURATION,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READER_MEDIASOURCE,
    MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
    MF_SOURCE_READERF_ERROR, MF_VERSION, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown, MFStartup,
    MFVideoArea, MFVideoFormat_RGB32,
};
use windows::core::{HSTRING, Interface};

use crate::{RecordError, Result};

#[allow(clippy::cast_sign_loss)]
const STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// Decodes the first video stream of a file into BGRA frames.
#[derive(Debug)]
pub struct VideoDecoder {
    reader: IMFSourceReader,
    /// Decoded buffer size.
    coded: (u32, u32),
    /// Visible part of the buffer (codecs pad to multiples of 16).
    visible: Rect,
    duration: Duration,
}

/// Reads the visible area from a media type.
fn geometry(media_type: &IMFMediaType) -> Result<((u32, u32), Rect)> {
    // SAFETY: plain attribute reads; the blob buffer matches its size.
    unsafe {
        let packed = media_type.GetUINT64(&MF_MT_FRAME_SIZE)?;
        #[allow(clippy::cast_possible_truncation)]
        let (w, h) = ((packed >> 32) as u32, packed as u32);
        #[allow(clippy::cast_possible_wrap)]
        let mut visible = Rect::new(0, 0, w as i32, h as i32);
        let mut area = MFVideoArea::default();
        let bytes =
            std::slice::from_raw_parts_mut((&raw mut area).cast::<u8>(), size_of::<MFVideoArea>());
        if media_type
            .GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, bytes, None)
            .is_ok()
        {
            let x = i32::from(area.OffsetX.value);
            let y = i32::from(area.OffsetY.value);
            let aperture = Rect::from_size(x, y, area.Area.cx, area.Area.cy);
            if !aperture.is_empty() {
                visible = aperture.intersect(visible);
            }
        }
        Ok(((w, h), visible))
    }
}

impl VideoDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        // SAFETY: Media Foundation calls with valid arguments; MFStartup is
        // balanced in `Drop`.
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
            let open = || -> Result<Self> {
                let mut attributes: Option<IMFAttributes> = None;
                MFCreateAttributes(&mut attributes, 2)?;
                let attributes = attributes.ok_or(RecordError::ThreadLost)?;
                attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
                attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
                let reader =
                    MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes)?;
                let rgb = MFCreateMediaType()?;
                rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
                reader.SetCurrentMediaType(STREAM, None, &rgb)?;
                let (coded, visible) = geometry(&reader.GetCurrentMediaType(STREAM)?)?;
                #[allow(clippy::cast_sign_loss)]
                let duration = reader
                    .GetPresentationAttribute(
                        MF_SOURCE_READER_MEDIASOURCE.0 as u32,
                        &MF_PD_DURATION,
                    )
                    .ok()
                    .and_then(|v| u64::try_from(&v).ok())
                    .map_or(Duration::ZERO, |ticks| Duration::from_nanos(ticks * 100));
                Ok(Self {
                    reader,
                    coded,
                    visible,
                    duration,
                })
            };
            open().inspect_err(|_| {
                let _ = MFShutdown();
            })
        }
    }

    /// Size of the decoded frames.
    #[allow(clippy::cast_sign_loss)]
    pub fn size(&self) -> (u32, u32) {
        (self.visible.width() as u32, self.visible.height() as u32)
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Next frame and its presentation time, or `None` at the end.
    pub fn next_frame(&mut self) -> Result<Option<(BgraImage, Duration)>> {
        loop {
            let (mut flags, mut time) = (0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            // SAFETY: valid out pointers.
            unsafe {
                self.reader.ReadSample(
                    STREAM,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )?;
            }
            #[allow(clippy::cast_sign_loss)]
            let has = |flag: i32| flags & flag as u32 != 0;
            if has(MF_SOURCE_READERF_ERROR.0) {
                return Err(RecordError::Windows(windows::core::Error::empty()));
            }
            if has(MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0) {
                // SAFETY: plain query.
                let media_type = unsafe { self.reader.GetCurrentMediaType(STREAM)? };
                (self.coded, self.visible) = geometry(&media_type)?;
            }
            if has(MF_SOURCE_READERF_ENDOFSTREAM.0) {
                return Ok(None);
            }
            let Some(sample) = sample else {
                continue;
            };
            #[allow(clippy::cast_sign_loss)]
            let at = Duration::from_nanos(time.max(0) as u64 * 100);
            return Ok(Some((self.copy(&sample)?, at)));
        }
    }

    fn copy(&self, sample: &IMFSample) -> Result<BgraImage> {
        let (w, h) = self.coded;
        let mut full = BgraImage::new(w, h);
        // SAFETY: the buffer is locked while read; the slices stay within
        // the locked area (`h` rows of `pitch` bytes).
        unsafe {
            let buffer = sample.GetBufferByIndex(0)?;
            if let Ok(buffer2d) = buffer.cast::<IMF2DBuffer>() {
                let (mut scan0, mut pitch) = (std::ptr::null_mut(), 0i32);
                buffer2d.Lock2D(&mut scan0, &mut pitch)?;
                let row_bytes = w as usize * 4;
                let dst = full.as_bytes_mut();
                for y in 0..h as usize {
                    #[allow(clippy::cast_possible_wrap)]
                    let row = scan0.offset(y as isize * pitch as isize);
                    let src = std::slice::from_raw_parts(row, row_bytes);
                    dst[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(src);
                }
                buffer2d.Unlock2D()?;
            } else {
                let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
                buffer.Lock(&mut data, None, Some(&mut len))?;
                let src = std::slice::from_raw_parts(data, len as usize);
                let n = src.len().min(full.as_bytes().len());
                full.as_bytes_mut()[..n].copy_from_slice(&src[..n]);
                buffer.Unlock()?;
            }
        }
        // RGB32 has no alpha.
        for px in full.as_bytes_mut().as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Ok(if self.visible == full.bounds() {
            full
        } else {
            full.crop(self.visible)
        })
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        // SAFETY: balances the MFStartup in `open`.
        let _ = unsafe { MFShutdown() };
    }
}

/// Per-channel noise ignored when converting decoded video.
const VIDEO_TOLERANCE: u8 = 6;

/// Result of [`video_to_gif`].
#[derive(Debug, Clone)]
pub struct Conversion {
    pub summary: GifSummary,
    /// The last frame, at most 320 × 200, for a preview.
    pub thumbnail: Option<BgraImage>,
}

/// Converts the video at `input` into a GIF at `output`, sampled at `fps`.
/// `progress` receives the fraction done (0–1) and returns `false` to
/// cancel.
pub fn video_to_gif(
    input: &Path,
    output: &Path,
    fps: u32,
    options: GifOptions,
    mut progress: impl FnMut(f32) -> bool,
) -> Result<Conversion> {
    let fps = fps.clamp(1, kadra_gif::MAX_FPS);
    let mut decoder = VideoDecoder::open(input)?;
    let (w, h) = decoder.size();
    let options = GifOptions {
        // Ignore compression noise.
        tolerance: options.tolerance.max(VIDEO_TOLERANCE),
        ..options
    };
    let mut writer = GifWriter::create(output, w, h, options)?;
    let total = decoder.duration();
    let mut origin = None;
    let mut last_slot = None;
    let mut end = Duration::ZERO;
    let mut last = None;

    while let Some((frame, at)) = decoder.next_frame()? {
        // Timestamps may not start at zero.
        let at = at.saturating_sub(*origin.get_or_insert(at));
        end = end.max(at);
        // Keep the first frame of each 1/fps slot.
        let slot = at.as_nanos() * u128::from(fps) / 1_000_000_000;
        if last_slot.is_some_and(|last| slot <= last) {
            continue;
        }
        last_slot = Some(slot);
        let time = Duration::from_nanos(
            u64::try_from(slot * 1_000_000_000 / u128::from(fps)).unwrap_or(u64::MAX),
        );
        writer.push(&frame, time)?;
        last = Some(frame);
        #[allow(clippy::cast_possible_truncation)]
        let done = if total.is_zero() {
            0.0
        } else {
            (at.as_secs_f64() / total.as_secs_f64()).min(1.0) as f32
        };
        if !progress(done) {
            drop(writer);
            let _ = std::fs::remove_file(output);
            return Err(RecordError::Cancelled);
        }
    }
    // The last frame lasts one source frame interval at most; use the
    // container duration when it is known.
    let end = if total > end {
        total
    } else {
        end + Duration::from_secs(1) / fps
    };
    let summary = writer.finish(end)?;
    progress(1.0);
    Ok(Conversion {
        summary,
        thumbnail: last.map(|frame| frame.thumbnail(320, 200)),
    })
}
