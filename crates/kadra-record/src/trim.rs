//! Editing a video: decode the kept segments and encode them again, one
//! after the other, optionally faster or slower, cropped and composited.
//!
//! ```text
//! hardware decoder ─▶ NV12 texture ─▶ video processor ─────────────────▶ NV12 ─▶ hardware encoder
//!                                      └▶ BGRA frame ─▶ Direct2D ─▶ video processor ─┘
//!                                         (canvas)      (overlays, cursor, clicks)
//! ```
//!
//! Frames never leave the GPU, so a recording exports many times faster
//! than real time. Re-encoding (rather than copying compressed samples)
//! keeps cuts frame-accurate: a copy could only start on a key frame.

use std::cell::RefCell;
use std::path::Path;
use std::time::Duration;

use kadra_capture::Gpu;
use kadra_core::{BgraImage, Rect, VideoCodec};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    ID3D11Texture2D, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC,
};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMF2DBuffer2, IMFAttributes, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFMediaType,
    IMFSample, IMFSinkWriter, IMFSourceReader, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_MPEG2_PROFILE, MF_MT_SUBTYPE,
    MF_MT_YUV_MATRIX, MF_PD_DURATION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SINK_WRITER_D3D_MANAGER, MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ANY_STREAM,
    MF_SOURCE_READER_D3D_MANAGER, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR,
    MF_VERSION, MF2DBuffer_LockFlags_Read, MFAudioFormat_AAC, MFAudioFormat_PCM,
    MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL,
    MFMediaType_Audio, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown, MFStartup, MFVideoArea,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoTransferMatrix_BT601, eAVEncH264VProfile_High,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING, Interface};

use crate::compose::{Composer, Composition};
use crate::convert::{Blitter, Space, render_texture};
use crate::cursor::CursorTrack;
use crate::encoder::{VideoSettings, set_input, video_type};
use crate::paint::Painter;
use crate::stretch::{apply_gain, click_sound, click_volume, mix_at, stretch};
use crate::{Part, RecordError, Result};

#[allow(clippy::cast_sign_loss)]
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
#[allow(clippy::cast_sign_loss)]
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
/// AAC at 192 kbit/s.
const AAC_BYTES_PER_SECOND: u32 = 24_000;
/// Encoder quality-vs-speed for exports: measured on an RTX 3080, 33 keeps
/// the quality of 100 (within 0.01 dB) and encodes 30% faster.
const EXPORT_EFFORT: u32 = 33;

#[derive(Debug, Clone)]
pub struct TrimOptions {
    /// Parts of the video to keep, in order, each at its own speed.
    pub segments: Vec<Part>,
    /// Keep the sound track (if there is one).
    pub audio: bool,
    /// 0 (best) to 51 (smallest); edited videos default to a high quality.
    pub quality: u8,
    /// Area to keep, in pixels of the video; `None` keeps the whole frame.
    pub crop: Option<Rect>,
    /// Drawn on the frames (overlays, cursor, clicks, canvas).
    pub composition: Composition,
    /// Cursor recorded with the video (needed for cursor and click effects).
    pub track: Option<CursorTrack>,
    /// Frame rate of the result; `None` (or more than the source) keeps
    /// the source rate.
    pub fps: Option<u32>,
    /// Height of the result at most (scaled down, never up).
    pub max_height: Option<u32>,
    /// Never encode below the source's own bitrate (no visible loss);
    /// off when the user picked the quality.
    pub match_source: bool,
    /// Volume and fades of the sound track.
    pub sound: SoundEdit,
}

/// Changes to the recorded sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundEdit {
    /// 1 keeps the recorded level; up to 2.
    pub volume: f32,
    /// Seconds over which the sound rises at the start, and falls at the end.
    pub fade_in: f64,
    pub fade_out: f64,
}

impl Default for SoundEdit {
    fn default() -> Self {
        Self {
            volume: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
        }
    }
}

impl SoundEdit {
    fn is_plain(&self) -> bool {
        (self.volume - 1.0).abs() < 1e-3 && self.fade_in <= 0.0 && self.fade_out <= 0.0
    }

    /// Level at `t` seconds of a result `total` seconds long.
    #[allow(clippy::cast_possible_truncation)]
    fn gain(&self, t: f64, total: f64) -> f32 {
        let mut g = f64::from(self.volume.clamp(0.0, 2.0));
        if self.fade_in > 0.0 {
            g *= (t / self.fade_in).clamp(0.0, 1.0);
        }
        if self.fade_out > 0.0 {
            g *= ((total - t) / self.fade_out).clamp(0.0, 1.0);
        }
        g as f32
    }
}

impl TrimOptions {
    /// The whole video, unchanged.
    pub fn whole(audio: bool, quality: u8) -> Self {
        Self {
            segments: vec![Part::whole()],
            audio,
            quality,
            crop: None,
            composition: Composition::default(),
            track: None,
            fps: None,
            max_height: None,
            match_source: true,
            sound: SoundEdit::default(),
        }
    }

    fn click_sound(&self) -> bool {
        self.composition.wants_click_sound() && self.track.is_some()
    }
}

/// `crop` (relative to the visible frame) as an even-aligned area of the
/// decoded frame.
fn apply_crop(visible: Rect, crop: Option<Rect>) -> Rect {
    let Some(crop) = crop else {
        return visible;
    };
    let area = crop.offset(visible.left, visible.top).intersect(visible);
    if area.width() < 2 || area.height() < 2 {
        return visible;
    }
    let even = |v: i32| v & !1;
    Rect::new(
        even(area.left),
        even(area.top),
        even(area.left) + even(area.width()),
        even(area.top) + even(area.height()),
    )
}

#[derive(Debug, Clone, Copy)]
pub struct TrimStats {
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub duration: Duration,
}

fn ticks(d: Duration) -> i64 {
    i64::try_from(d.as_nanos() / 100).unwrap_or(i64::MAX)
}

/// Decoded NV12 frame layout.
#[derive(Debug, Clone, Copy)]
struct Layout {
    coded: (u32, u32),
    visible: Rect,
}

fn layout(media_type: &IMFMediaType) -> Result<Layout> {
    // SAFETY: plain attribute reads; the blob matches the structure size.
    unsafe {
        let packed = media_type.GetUINT64(&MF_MT_FRAME_SIZE)?;
        #[allow(clippy::cast_possible_truncation)]
        let coded = ((packed >> 32) as u32, packed as u32);
        #[allow(clippy::cast_possible_wrap)]
        let mut visible = Rect::new(0, 0, coded.0 as i32, coded.1 as i32);
        let mut area = MFVideoArea::default();
        let bytes =
            std::slice::from_raw_parts_mut((&raw mut area).cast::<u8>(), size_of::<MFVideoArea>());
        if media_type
            .GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, bytes, None)
            .is_ok()
        {
            let a = Rect::from_size(
                i32::from(area.OffsetX.value),
                i32::from(area.OffsetY.value),
                area.Area.cx,
                area.Area.cy,
            );
            if !a.is_empty() {
                visible = a.intersect(visible);
            }
        }
        // NV12 needs even dimensions and offsets.
        let even = |v: i32| v & !1;
        visible = Rect::new(
            even(visible.left),
            even(visible.top),
            even(visible.left) + even(visible.width()),
            even(visible.top) + even(visible.height()),
        );
        Ok(Layout { coded, visible })
    }
}

fn audio_type(subtype: &GUID, rate: u32, channels: u32) -> windows::core::Result<IMFMediaType> {
    // SAFETY: attribute setters on a fresh media type.
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        Ok(t)
    }
}

/// Composited frames: the decoded video is drawn into a BGRA frame (over
/// the canvas background), painted over, then converted for the encoder.
struct Composed {
    /// Decoded NV12 → BGRA frame.
    decode: Blitter,
    frame: ID3D11Texture2D,
    frame_view: ID3D11VideoProcessorOutputView,
    background: Option<ID3D11Texture2D>,
    painter: Painter,
    /// BGRA frame → NV12 for the encoder.
    encode: Blitter,
    frame_input: ID3D11VideoProcessorInputView,
    /// Where the video goes in the frame.
    area: Rect,
    /// The whole frame.
    full: Rect,
}

/// Turns decoded frames into encoder input, on the GPU.
struct Frames {
    gpu: Gpu,
    /// Source area kept, in decoded pixels.
    crop: Rect,
    /// Output size (even).
    size: (u32, u32),
    /// Plain cuts: decoded NV12 → cropped NV12.
    direct: Option<Blitter>,
    composed: Option<Composed>,
    /// Holds frames from decoders that only work in system memory.
    upload: RefCell<Option<ID3D11Texture2D>>,
    layout: Layout,
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
impl Frames {
    fn new(
        gpu: &Gpu,
        layout: Layout,
        space: Space,
        fps: u32,
        options: &TrimOptions,
    ) -> Result<Self> {
        let visible = layout.visible;
        let crop = apply_crop(visible, options.crop);
        let crop_size = (crop.width() as u32, crop.height() as u32);
        let canvas = options
            .composition
            .canvas
            .as_ref()
            .map(|style| Composer::canvas(style, crop_size.0, crop_size.1));
        // The frame as composed, then the size written (scaled down to the
        // height asked, if any).
        let natural = canvas.as_ref().map_or(crop_size, |(background, _)| {
            (background.width() & !1, background.height() & !1)
        });
        let size = output_size(natural, options.max_height);
        let full = Rect::new(0, 0, natural.0 as i32, natural.1 as i32);
        let (direct, composed) = if options.composition.is_empty() {
            let blit = Blitter::new(gpu, layout.coded, size, fps, (space, Space::Bt709))?;
            (Some(blit), None)
        } else {
            let area = canvas.as_ref().map_or(full, |(_, area)| *area);
            let frame = render_texture(gpu, natural, DXGI_FORMAT_B8G8R8A8_UNORM)?;
            let decode = Blitter::new(gpu, layout.coded, natural, fps, (space, Space::Rgb))?;
            let frame_view = decode.output(&frame)?;
            let corners = options
                .composition
                .canvas
                .as_ref()
                .zip(canvas.as_ref())
                .map_or_else(Vec::new, |(style, (image, area))| {
                    Composer::corners(style, image, *area)
                });
            let background = canvas
                .map(|(image, _)| upload_bgra(gpu, &image))
                .transpose()?;
            let painter = Painter::new(
                gpu,
                &frame,
                &options.composition,
                options.track.clone(),
                // Cursor and overlays are placed relative to the visible
                // frame.
                crop.offset(-visible.left, -visible.top),
                area,
                &corners,
            )?;
            let encode = Blitter::new(gpu, natural, size, fps, (Space::Rgb, Space::Bt709))?;
            let frame_input = encode.input(&frame, 0)?;
            let composed = Composed {
                decode,
                frame,
                frame_view,
                background,
                painter,
                encode,
                frame_input,
                area,
                full,
            };
            (None, Some(composed))
        };
        Ok(Self {
            gpu: gpu.clone(),
            crop,
            size,
            direct,
            composed,
            upload: RefCell::new(None),
            layout,
        })
    }

    /// The decoded texture of `sample` (uploaded first if the decoder
    /// works in system memory).
    fn source(&self, sample: &IMFSample) -> Result<(ID3D11Texture2D, u32)> {
        // SAFETY: COM calls on a live sample.
        unsafe {
            let buffer = sample.GetBufferByIndex(0)?;
            if let Ok(dxgi) = buffer.cast::<IMFDXGIBuffer>() {
                let mut texture = std::ptr::null_mut();
                dxgi.GetResource(&ID3D11Texture2D::IID, &mut texture)?;
                let texture = ID3D11Texture2D::from_raw(texture);
                return Ok((texture, dxgi.GetSubresourceIndex()?));
            }
        }
        let mut upload = self.upload.borrow_mut();
        if upload.is_none() {
            *upload = Some(render_texture(
                &self.gpu,
                self.layout.coded,
                DXGI_FORMAT_NV12,
            )?);
        }
        let texture = upload.clone().ok_or(RecordError::ThreadLost)?;
        let nv12 = packed_nv12(sample, self.layout.coded)?;
        // SAFETY: `nv12` holds the whole frame at a pitch of the width.
        unsafe {
            self.gpu.context().UpdateSubresource(
                &texture,
                0,
                None,
                nv12.as_ptr().cast(),
                self.layout.coded.0,
                0,
            );
        }
        Ok((texture, 0))
    }

    /// Renders one frame (source time `t` in seconds) for the encoder.
    fn render(&self, sample: &IMFSample, t: f64) -> Result<ID3D11Texture2D> {
        let (texture, slice) = self.source(sample)?;
        let output = render_texture(&self.gpu, self.size, DXGI_FORMAT_NV12)?;
        let full = Rect::new(0, 0, self.size.0 as i32, self.size.1 as i32);
        if let Some(blit) = &self.direct {
            let input = blit.input(&texture, slice)?;
            blit.blit(&input, self.crop, &blit.output(&output)?, full)?;
        } else if let Some(c) = &self.composed {
            if let Some(background) = &c.background {
                // SAFETY: same size and format.
                unsafe { self.gpu.context().CopyResource(&c.frame, background) };
            }
            let input = c.decode.input(&texture, slice)?;
            c.decode.blit(&input, self.crop, &c.frame_view, c.area)?;
            c.painter.paint(t)?;
            c.encode
                .blit(&c.frame_input, c.full, &c.encode.output(&output)?, full)?;
        }
        Ok(output)
    }
}

/// `size` scaled down to `max_height` (even dimensions); never enlarged.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn output_size(size: (u32, u32), max_height: Option<u32>) -> (u32, u32) {
    match max_height {
        Some(h) if h >= 2 && h < size.1 => {
            let scale = f64::from(h) / f64::from(size.1);
            (
                ((f64::from(size.0) * scale).round() as u32).max(2) & !1,
                h & !1,
            )
        }
        _ => size,
    }
}

/// A BGRA image as a texture.
fn upload_bgra(gpu: &Gpu, image: &BgraImage) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: image.width() & !1,
        Height: image.height() & !1,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        #[allow(clippy::cast_sign_loss)]
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let data = D3D11_SUBRESOURCE_DATA {
        pSysMem: image.as_bytes().as_ptr().cast(),
        SysMemPitch: image.width() * 4,
        SysMemSlicePitch: 0,
    };
    let mut texture = None;
    // SAFETY: `data` covers the whole texture.
    unsafe {
        gpu.device()
            .CreateTexture2D(&desc, Some(&data), Some(&mut texture))?;
    }
    Ok(texture.ok_or_else(windows::core::Error::empty)?)
}

/// A decoded NV12 sample in system memory, as a packed frame of the coded
/// size (luma rows then chroma rows, `width` bytes each).
fn packed_nv12(sample: &IMFSample, (w, h): (u32, u32)) -> Result<Vec<u8>> {
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 3 / 2];
    // SAFETY: rows are read within the locked buffer.
    unsafe {
        let src = sample.ConvertToContiguousBuffer()?;
        let mut copy = |scan0: *const u8, pitch: usize, length: usize| {
            // Decoders may pad the luma plane: the chroma plane starts
            // after `rows` rows, which may exceed the frame height.
            let rows = (length / pitch.max(1)) * 2 / 3;
            let rows = if rows >= h {
                rows
            } else {
                h.next_multiple_of(16)
            };
            let width = w.min(pitch);
            for row in 0..h {
                let from = std::slice::from_raw_parts(scan0.add(row * pitch), width);
                out[row * w..row * w + width].copy_from_slice(from);
            }
            for row in 0..h / 2 {
                let from = std::slice::from_raw_parts(scan0.add((rows + row) * pitch), width);
                let at = w * h + row * w;
                out[at..at + width].copy_from_slice(from);
            }
        };
        if let Ok(buffer2d) = src.cast::<IMF2DBuffer2>() {
            let (mut scan0, mut pitch) = (std::ptr::null_mut(), 0i32);
            let (mut start, mut length) = (std::ptr::null_mut(), 0u32);
            buffer2d.Lock2DSize(
                MF2DBuffer_LockFlags_Read,
                &mut scan0,
                &mut pitch,
                &mut start,
                &mut length,
            )?;
            copy(scan0, pitch.unsigned_abs() as usize, length as usize);
            buffer2d.Unlock2D()?;
        } else {
            let (mut data, mut length) = (std::ptr::null_mut(), 0u32);
            src.Lock(&mut data, None, Some(&mut length))?;
            copy(data, w, length as usize);
            src.Unlock()?;
        }
    }
    Ok(out)
}

struct Pipeline {
    reader: IMFSourceReader,
    writer: IMFSinkWriter,
    frames: Frames,
    video_out: u32,
    /// Writer stream for the sound, if kept.
    audio_out: Option<u32>,
    /// Sample rate and channels of the sound written.
    audio_format: (u32, u32),
    /// The source has no sound: the track is generated (click sounds).
    synth_audio: bool,
    duration: Duration,
    fps: u32,
    _manager: IMFDXGIDeviceManager,
}

/// Bitrate that keeps what the source has: re-encoding below the source's
/// own bitrate would lose detail the recording kept.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn export_bitrate(settings: &VideoSettings, input: &Path, duration: Duration, pixels: f64) -> u32 {
    let source = std::fs::metadata(input).map_or(0, |m| m.len()) as f64 * 8.0
        / duration.as_secs_f64().max(0.1);
    let wanted = source * 1.15 * pixels.clamp(0.25, 4.0);
    f64::from(settings.bitrate()).max(wanted).min(200_000_000.0) as u32
}

/// Opens the reader and the writer.
///
/// SAFETY: Media Foundation must be started.
#[allow(clippy::too_many_lines)]
unsafe fn open(input: &Path, output: &Path, options: &TrimOptions) -> Result<Pipeline> {
    // SAFETY: Media Foundation calls with valid arguments.
    unsafe {
        let gpu = Gpu::new()?;
        let mut token = 0;
        let mut manager = None;
        MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
        let manager = manager.ok_or(RecordError::ThreadLost)?;
        manager.ResetDevice(gpu.device(), token)?;

        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 3)?;
        let attributes = attributes.ok_or(RecordError::ThreadLost)?;
        attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &manager)?;
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
        attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(input.as_os_str()), &attributes)?;
        #[allow(clippy::cast_sign_loss)]
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(VIDEO, true)?;

        let wanted = MFCreateMediaType()?;
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        reader.SetCurrentMediaType(VIDEO, None, &wanted)?;
        let decoded = reader.GetCurrentMediaType(VIDEO)?;
        let layout = layout(&decoded)?;
        #[allow(clippy::cast_possible_wrap)]
        let space = match decoded.GetUINT32(&MF_MT_YUV_MATRIX) {
            Ok(m) if m == MFVideoTransferMatrix_BT601.0 as u32 => Space::Bt601,
            _ => Space::Bt709,
        };
        let fps = decoded
            .GetUINT64(&MF_MT_FRAME_RATE)
            .ok()
            .map(|r| {
                #[allow(clippy::cast_possible_truncation)]
                let (num, den) = ((r >> 32) as u32, (r as u32).max(1));
                (num + den / 2) / den
            })
            .filter(|&f| f > 0)
            .unwrap_or(30);
        let frames = Frames::new(&gpu, layout, space, fps, options)?;
        // A lower frame rate asked for drops frames; never a higher one.
        let out_fps = options.fps.map_or(fps, |f| f.clamp(1, fps));

        #[allow(clippy::cast_sign_loss)]
        let duration = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ok()
            .and_then(|v| u64::try_from(&v).ok())
            .map_or(Duration::ZERO, |t| Duration::from_nanos(t * 100));

        let mut writer_attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut writer_attributes, 2)?;
        let writer_attributes = writer_attributes.ok_or(RecordError::ThreadLost)?;
        writer_attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        writer_attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)?;
        let writer = MFCreateSinkWriterFromURL(
            &HSTRING::from(output.as_os_str()),
            None,
            &writer_attributes,
        )?;
        let (out_w, out_h) = frames.size;
        let settings = VideoSettings {
            width: out_w,
            height: out_h,
            fps: out_fps,
            codec: VideoCodec::H264,
            quality: options.quality,
        };
        #[allow(clippy::cast_precision_loss)]
        let pixels = f64::from(out_w) * f64::from(out_h)
            / (f64::from(layout.visible.width().max(1))
                * f64::from(layout.visible.height().max(1)));
        let bitrate = if options.match_source {
            export_bitrate(&settings, input, duration, pixels)
        } else {
            settings.bitrate()
        };
        let encoded = video_type(&settings, &MFVideoFormat_H264)?;
        encoded.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        #[allow(clippy::cast_sign_loss)]
        encoded.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
        let video_out = writer.AddStream(&encoded)?;
        set_input(&writer, video_out, &settings, bitrate, EXPORT_EFFORT)?;

        let mut audio_format = (48_000, 2);
        let audio_out = if options.audio && reader.SetStreamSelection(AUDIO, true).is_ok() {
            let pcm = MFCreateMediaType()?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            let configured = reader
                .SetCurrentMediaType(AUDIO, None, &pcm)
                .and_then(|()| {
                    let current = reader.GetCurrentMediaType(AUDIO)?;
                    let rate = current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
                    let channels = current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)?;
                    let aac = audio_type(&MFAudioFormat_AAC, rate, channels)?;
                    aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
                    let stream = writer.AddStream(&aac)?;
                    writer.SetInputMediaType(stream, &current, None)?;
                    Ok((stream, rate, channels))
                });
            match configured {
                Ok((stream, rate, channels)) => {
                    audio_format = (rate, channels);
                    Some(stream)
                }
                Err(err) => {
                    // AAC only takes 44.1/48 kHz, mono or stereo.
                    tracing::warn!("sound dropped: {err}");
                    let _ = reader.SetStreamSelection(AUDIO, false);
                    None
                }
            }
        } else {
            None
        };
        // No sound in the source but click sounds wanted: generate a track.
        let mut synth_audio = false;
        let audio_out = match audio_out {
            None if options.click_sound() => {
                let pcm = audio_type(&MFAudioFormat_PCM, 48_000, 2)?;
                pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
                pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 48_000 * 4)?;
                let aac = audio_type(&MFAudioFormat_AAC, 48_000, 2)?;
                aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
                let stream = writer.AddStream(&aac)?;
                writer.SetInputMediaType(stream, &pcm, None)?;
                synth_audio = true;
                audio_format = (48_000, 2);
                Some(stream)
            }
            other => other,
        };
        Ok(Pipeline {
            reader,
            writer,
            frames,
            video_out,
            audio_out,
            audio_format,
            synth_audio,
            duration,
            fps: out_fps,
            _manager: manager,
        })
    }
}

/// Writes the kept segments of `input` to `output` (H.264 + AAC MP4).
/// `progress` receives the fraction done and returns `false` to cancel.
pub fn trim_video(
    input: &Path,
    output: &Path,
    options: &TrimOptions,
    mut progress: impl FnMut(f32) -> bool,
) -> Result<TrimStats> {
    // SAFETY: Media Foundation calls with valid arguments; MFStartup is
    // balanced below.
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
    // SAFETY: Media Foundation is started.
    let result = unsafe { run(input, output, options, &mut progress) };
    // SAFETY: balances MFStartup; every Media Foundation object is gone.
    let _ = unsafe { MFShutdown() };
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

/// Parts clipped to the video, in 100 ns units and in order, with their
/// speed.
fn clip_segments(parts: &[Part], duration: Duration) -> Vec<(i64, i64, f64)> {
    let limit = if duration.is_zero() {
        i64::MAX
    } else {
        ticks(duration)
    };
    let mut out: Vec<(i64, i64, f64)> = parts
        .iter()
        .map(|p| {
            (
                ticks(p.start).min(limit),
                ticks(p.end).min(limit),
                p.speed(),
            )
        })
        .filter(|(a, b, _)| b > a)
        .collect();
    out.sort_unstable_by_key(|s| s.0);
    out
}

/// Writes an NV12 texture to the encoder.
fn write_texture(
    writer: &IMFSinkWriter,
    stream: u32,
    texture: &ID3D11Texture2D,
    time: i64,
    duration: i64,
) -> Result<()> {
    // SAFETY: the texture is an NV12 surface of the writer's device.
    unsafe {
        let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)?;
        let length = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
        buffer.SetCurrentLength(length)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(time)?;
        sample.SetSampleDuration(duration)?;
        writer.WriteSample(stream, &sample)?;
    }
    Ok(())
}

/// SAFETY: Media Foundation must be started.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
unsafe fn run(
    input: &Path,
    output: &Path,
    options: &TrimOptions,
    progress: &mut impl FnMut(f32) -> bool,
) -> Result<TrimStats> {
    // SAFETY: Media Foundation calls with valid arguments.
    unsafe {
        let p = open(input, output, options)?;
        let segments = clip_segments(&options.segments, p.duration);
        if segments.is_empty() {
            return Err(RecordError::EmptyRegion);
        }
        let total: i64 = segments
            .iter()
            .map(|(a, b, _)| (*b).min(ticks(p.duration).max(*a + 1)) - a)
            .sum::<i64>()
            .max(1);
        // Faster playback keeps the source frame rate: frames closer than
        // this to the previous one are dropped rather than encoded.
        let frame_ticks = 10_000_000 / i64::from(p.fps.max(1));
        let min_gap = frame_ticks * 9 / 10;
        p.writer.BeginWriting()?;

        #[allow(clippy::cast_sign_loss)]
        let any = MF_SOURCE_READER_ANY_STREAM.0 as u32;
        // The sound is rebuilt (time-stretched, clicks mixed in) rather
        // than copied when the speed changes or clicks are heard. It is
        // prepared per segment from a second reader, then written as the
        // video progresses: the writer holds back a stream that runs ahead.
        let rebuild_audio = p.audio_out.is_some()
            && !p.synth_audio
            && (segments.iter().any(|s| (s.2 - 1.0).abs() > 1e-6)
                || options.click_sound()
                || !options.sound.is_plain());
        let copy_audio = p.audio_out.is_some() && !p.synth_audio && !rebuild_audio;
        let audio_reader = if rebuild_audio {
            Some(AudioReader::open(input)?)
        } else {
            None
        };
        let clicks: Vec<f64> = if options.click_sound() {
            options
                .track
                .as_ref()
                .map_or_else(Vec::new, |t| t.clicks.iter().map(|c| c.t).collect())
        } else {
            Vec::new()
        };
        let voice = options
            .composition
            .clicks
            .as_ref()
            .map_or_else(Default::default, |c| c.voice);
        let click = click_sound(p.audio_format.0, voice);
        // Length of the result, for the fade out.
        let output_seconds: f64 = segments
            .iter()
            .map(|&(start, end, speed)| {
                (end.min(ticks(p.duration).max(start + 1)) - start) as f64 / 1e7 / speed
            })
            .sum();
        let (rate, channels) = (p.audio_format.0, p.audio_format.1 as usize);
        let mut frames = 0u64;
        // Output time where the current segment starts, and source time
        // already written (for progress).
        let mut offset = 0i64;
        let mut done_before = 0i64;
        let mut last_written = i64::MIN;
        for &(start, end, speed) in &segments {
            let scale = |t: i64| (t as f64 / speed).round() as i64;
            p.reader.SetStreamSelection(VIDEO, true)?;
            if copy_audio {
                p.reader.SetStreamSelection(AUDIO, true)?;
            }
            // Sound made for this segment, written along with the video.
            let mut pending = match (p.audio_out, &audio_reader) {
                (Some(stream), Some(reader)) if rebuild_audio => {
                    let pcm = reader.read(start, end, rate, channels)?;
                    let mut pcm = stretch(&pcm, channels, rate, speed);
                    if !options.sound.is_plain() {
                        let at = offset as f64 / 1e7;
                        apply_gain(&mut pcm, channels, rate, |t| {
                            options.sound.gain(at + t, output_seconds)
                        });
                    }
                    Some(PendingPcm::new(pcm, stream, offset))
                }
                (Some(stream), _) if p.synth_audio => {
                    let length = end.min(ticks(p.duration).max(start + 1)) - start;
                    let frames = (scale(length) as f64 / 1e7 * f64::from(rate)) as usize;
                    Some(PendingPcm::new(vec![0; frames * channels], stream, offset))
                }
                _ => None,
            };
            if let Some(pending) = &mut pending {
                let here: Vec<f64> = clicks
                    .iter()
                    .filter(|&&t| t >= start as f64 / 1e7 && t < end as f64 / 1e7)
                    .map(|&t| (t - start as f64 / 1e7) / speed)
                    .collect();
                mix_at(
                    &mut pending.pcm,
                    channels,
                    rate,
                    &click,
                    &here,
                    click_volume(voice),
                );
            }
            // Lands on the key frame before `start`; earlier frames are
            // decoded and dropped below.
            p.reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(start))?;
            let (mut video_done, mut audio_done) = (false, !copy_audio);
            let mut segment_end = offset;
            while !(video_done && audio_done) {
                let (mut index, mut flags, mut time) = (0u32, 0u32, 0i64);
                let mut sample: Option<IMFSample> = None;
                p.reader.ReadSample(
                    any,
                    0,
                    Some(&mut index),
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )?;
                #[allow(clippy::cast_sign_loss)]
                let has = |flag: i32| flags & flag as u32 != 0;
                if has(MF_SOURCE_READERF_ERROR.0) {
                    return Err(RecordError::Windows(windows::core::Error::empty()));
                }
                let is_video = !is_audio_index(&p.reader, index);
                if has(MF_SOURCE_READERF_ENDOFSTREAM.0) || time >= end {
                    if is_video {
                        video_done = true;
                    } else {
                        audio_done = true;
                    }
                    // Stop reading a stream that is past the end.
                    let _ = p.reader.SetStreamSelection(index, false);
                    continue;
                }
                let Some(sample) = sample else {
                    continue;
                };
                let duration = sample.GetSampleDuration().unwrap_or(frame_ticks);
                if time + duration <= start {
                    continue;
                }
                let at = offset + scale((time - start).max(0));
                if is_video {
                    segment_end = segment_end.max(at + scale(duration));
                    if last_written == i64::MIN || at - last_written >= min_gap {
                        let texture = p.frames.render(&sample, time as f64 / 1e7)?;
                        drop(sample);
                        if let Some(pending) = &mut pending {
                            pending.write_until(&p.writer, at + 5_000_000, channels, rate)?;
                        }
                        write_texture(&p.writer, p.video_out, &texture, at, scale(duration))?;
                        last_written = at;
                        frames += 1;
                    }
                    let done = (done_before + (time - start).max(0)) as f64 / total as f64;
                    if !progress(done.min(1.0) as f32) {
                        return Err(RecordError::Cancelled);
                    }
                } else if let Some(stream) = p.audio_out {
                    sample.SetSampleTime(at)?;
                    p.writer.WriteSample(stream, &sample)?;
                }
            }
            if let Some(pending) = &mut pending {
                pending.write_until(&p.writer, i64::MAX, channels, rate)?;
            }
            done_before += end.min(ticks(p.duration).max(start + 1)) - start;
            // The next segment follows the last frame written.
            offset = offset.max(segment_end);
        }
        if frames == 0 {
            return Err(RecordError::EmptyRegion);
        }
        p.writer.Finalize()?;
        progress(1.0);
        let (width, height) = p.frames.size;
        Ok(TrimStats {
            width,
            height,
            frames,
            duration: Duration::from_nanos(offset as u64 * 100),
        })
    }
}

/// Interleaved 16-bit samples of a decoded PCM sample.
fn read_pcm(sample: &IMFSample) -> Result<Vec<i16>> {
    // SAFETY: the buffer is locked while read.
    unsafe {
        let buffer = sample.ConvertToContiguousBuffer()?;
        let (mut data, mut len) = (std::ptr::null_mut(), 0u32);
        buffer.Lock(&mut data, None, Some(&mut len))?;
        let bytes = std::slice::from_raw_parts(data, len as usize);
        let pcm = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect();
        buffer.Unlock()?;
        Ok(pcm)
    }
}

/// Reads the sound of the source as 16-bit PCM, for rebuilding it.
struct AudioReader {
    reader: IMFSourceReader,
}

impl AudioReader {
    fn open(input: &Path) -> Result<Self> {
        // SAFETY: Media Foundation calls with valid arguments.
        unsafe {
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(input.as_os_str()), None)?;
            #[allow(clippy::cast_sign_loss)]
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(AUDIO, true)?;
            let pcm = MFCreateMediaType()?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            reader.SetCurrentMediaType(AUDIO, None, &pcm)?;
            Ok(Self { reader })
        }
    }

    /// Interleaved samples from `start` to `end` (100 ns units).
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn read(&self, start: i64, end: i64, rate: u32, channels: usize) -> Result<Vec<i16>> {
        let mut out = Vec::new();
        // SAFETY: Media Foundation calls with valid arguments.
        unsafe {
            self.reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(start))?;
            loop {
                let (mut flags, mut time) = (0u32, 0i64);
                let mut sample: Option<IMFSample> = None;
                self.reader.ReadSample(
                    AUDIO,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )?;
                #[allow(clippy::cast_sign_loss)]
                let stop = flags
                    & (MF_SOURCE_READERF_ENDOFSTREAM.0 | MF_SOURCE_READERF_ERROR.0) as u32
                    != 0;
                if stop || time >= end {
                    break;
                }
                let Some(sample) = sample else {
                    continue;
                };
                let pcm = read_pcm(&sample)?;
                // Seeking lands before `start`: skip what precedes it.
                let skip = if time < start {
                    ((start - time) as f64 / 1e7 * f64::from(rate)) as usize * channels
                } else {
                    0
                };
                out.extend_from_slice(pcm.get(skip..).unwrap_or_default());
            }
        }
        // Nothing past `end`.
        let length = ((end - start) as f64 / 1e7 * f64::from(rate)) as usize * channels;
        out.truncate(length);
        Ok(out)
    }
}

/// Sound prepared for a segment, written in step with the video.
struct PendingPcm {
    pcm: Vec<i16>,
    stream: u32,
    /// Output time of the first sample (100 ns units).
    start: i64,
    /// Samples (per channel) already written.
    written: usize,
}

impl PendingPcm {
    fn new(pcm: Vec<i16>, stream: u32, start: i64) -> Self {
        Self {
            pcm,
            stream,
            start,
            written: 0,
        }
    }

    /// Writes the samples that start before output time `until`, in
    /// chunks the encoder takes comfortably.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn write_until(
        &mut self,
        writer: &IMFSinkWriter,
        until: i64,
        channels: usize,
        rate: u32,
    ) -> Result<()> {
        const CHUNK: usize = 4096;
        let channels = channels.max(1);
        let total = self.pcm.len() / channels;
        let time_of = |frame: usize| self.start + frame as i64 * 10_000_000 / i64::from(rate);
        while self.written < total && time_of(self.written) < until {
            let frames = CHUNK.min(total - self.written);
            let chunk = &self.pcm[self.written * channels..(self.written + frames) * channels];
            let bytes = std::mem::size_of_val(chunk);
            // SAFETY: the buffer is locked while `bytes` bytes are copied in.
            unsafe {
                let buffer = MFCreateMemoryBuffer(bytes as u32)?;
                let mut data = std::ptr::null_mut();
                buffer.Lock(&mut data, None, None)?;
                std::ptr::copy_nonoverlapping(chunk.as_ptr().cast::<u8>(), data, bytes);
                buffer.Unlock()?;
                buffer.SetCurrentLength(bytes as u32)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buffer)?;
                sample.SetSampleTime(time_of(self.written))?;
                sample.SetSampleDuration(frames as i64 * 10_000_000 / i64::from(rate))?;
                writer.WriteSample(self.stream, &sample)?;
            }
            self.written += frames;
        }
        Ok(())
    }
}

/// Whether reader stream `index` is the sound track.
fn is_audio_index(reader: &IMFSourceReader, index: u32) -> bool {
    // SAFETY: plain query.
    unsafe {
        reader
            .GetCurrentMediaType(index)
            .and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE))
            .is_ok_and(|major| major == MFMediaType_Audio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: f64) -> Duration {
        Duration::from_secs_f64(v)
    }

    #[test]
    fn segments_are_clipped_and_sorted() {
        let part = |a, b, speed| Part {
            start: s(a),
            end: s(b),
            speed,
        };
        let clipped = clip_segments(
            &[
                part(5.0, 9.0, 2.0),
                part(1.0, 2.0, 1.0),
                part(3.0, 3.0, 1.0),
            ],
            s(8.0),
        );
        assert_eq!(
            clipped,
            vec![(10_000_000, 20_000_000, 1.0), (50_000_000, 80_000_000, 2.0)]
        );
        assert_eq!(
            clip_segments(&[Part::whole()], s(2.0)),
            vec![(0, 20_000_000, 1.0)]
        );
    }

    #[test]
    fn crop_is_even_and_inside_the_frame() {
        let visible = Rect::new(0, 0, 1920, 1080);
        assert_eq!(apply_crop(visible, None), visible);
        assert_eq!(
            apply_crop(visible, Some(Rect::new(101, 51, 600, 2000))),
            Rect::new(100, 50, 598, 1078)
        );
        // Degenerate crops keep the whole frame.
        assert_eq!(apply_crop(visible, Some(Rect::new(5, 5, 6, 6))), visible);
    }

    #[test]
    fn exports_keep_at_least_the_source_bitrate() {
        let settings = VideoSettings {
            width: 1920,
            height: 1080,
            fps: 60,
            codec: VideoCodec::H264,
            quality: 18,
        };
        // A missing file has no bitrate: the quality decides.
        let none = export_bitrate(&settings, Path::new("missing.mp4"), s(10.0), 1.0);
        assert_eq!(none, settings.bitrate());
    }
}
