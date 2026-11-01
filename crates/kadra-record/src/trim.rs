//! Trimming a video: decode the kept range and encode it again.
//!
//! Re-encoding (rather than copying compressed samples) makes the cut
//! frame-accurate: a copy could only start on a key frame, possibly seconds
//! before the chosen point. Encoding runs on the hardware encoder, so a
//! recording trims faster than real time.

use std::path::Path;
use std::time::Duration;

use kadra_core::{Rect, VideoCodec};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer2, IMFAttributes, IMFMediaType, IMFSample, IMFSinkWriter, IMFSourceReader,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_NUM_CHANNELS,
    MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_MPEG2_PROFILE, MF_MT_SUBTYPE,
    MF_PD_DURATION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ANY_STREAM, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR,
    MF_VERSION, MF2DBuffer_LockFlags_Read, MFAudioFormat_AAC, MFAudioFormat_PCM,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFMediaType_Video,
    MFSTARTUP_FULL, MFShutdown, MFStartup, MFVideoArea, MFVideoFormat_H264, MFVideoFormat_NV12,
    eAVEncH264VProfile_High,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING, Interface};

use crate::encoder::{VideoSettings, video_type};
use crate::{RecordError, Result};

#[allow(clippy::cast_sign_loss)]
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
#[allow(clippy::cast_sign_loss)]
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
/// AAC at 192 kbit/s.
const AAC_BYTES_PER_SECOND: u32 = 24_000;

#[derive(Debug, Clone, Copy)]
pub struct TrimOptions {
    pub start: Duration,
    /// `None` keeps everything after `start`.
    pub end: Option<Duration>,
    /// Keep the sound track (if there is one).
    pub audio: bool,
    /// 0 (best) to 51 (smallest); edited videos default to a high quality.
    pub quality: u8,
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

/// Copies the visible part of a decoded NV12 sample into a tightly packed
/// NV12 sample for the encoder.
fn crop_nv12(sample: &IMFSample, layout: Layout) -> Result<IMFSample> {
    let (cw, ch) = layout.coded;
    let visible = layout.visible;
    #[allow(clippy::cast_sign_loss, clippy::many_single_char_names)]
    let (w, h, x, y) = (
        visible.width() as usize,
        visible.height() as usize,
        visible.left as usize,
        visible.top as usize,
    );
    let size = w * h * 3 / 2;
    // SAFETY: source rows are read within the locked buffer (`rows` luma
    // rows then the chroma rows, `pitch` bytes each); the destination
    // buffer is `size` bytes.
    unsafe {
        let src = sample.ConvertToContiguousBuffer()?;
        #[allow(clippy::cast_possible_truncation)]
        let dst = MFCreateMemoryBuffer(size as u32)?;
        let mut out = std::ptr::null_mut();
        dst.Lock(&mut out, None, None)?;
        let out = std::slice::from_raw_parts_mut(out, size);

        // Decoders pad the luma plane (usually to a multiple of 16 rows):
        // the chroma plane starts after `rows`, not after the frame height.
        let mut copy = |scan0: *const u8, pitch: usize, rows: usize| {
            for row in 0..h {
                let from = scan0.add((y + row) * pitch + x);
                out[row * w..(row + 1) * w].copy_from_slice(std::slice::from_raw_parts(from, w));
            }
            let chroma = scan0.add(pitch * rows);
            for row in 0..h / 2 {
                let from = chroma.add((y / 2 + row) * pitch + x);
                let at = w * h + row * w;
                out[at..at + w].copy_from_slice(std::slice::from_raw_parts(from, w));
            }
        };
        let padded = (ch as usize).next_multiple_of(16);
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
            let pitch = pitch.unsigned_abs() as usize;
            // NV12: the luma plane is two thirds of the buffer.
            let rows = (length as usize / pitch.max(1)) * 2 / 3;
            let rows = if rows >= ch as usize { rows } else { padded };
            copy(scan0, pitch, rows);
            buffer2d.Unlock2D()?;
        } else {
            let (mut data, mut length) = (std::ptr::null_mut(), 0u32);
            src.Lock(&mut data, None, Some(&mut length))?;
            let pitch = cw as usize;
            let rows = (length as usize / pitch.max(1)) * 2 / 3;
            copy(data, pitch, if rows >= ch as usize { rows } else { padded });
            src.Unlock()?;
        }
        dst.Unlock()?;
        #[allow(clippy::cast_possible_truncation)]
        dst.SetCurrentLength(size as u32)?;
        let out = MFCreateSample()?;
        out.AddBuffer(&dst)?;
        Ok(out)
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

struct Pipeline {
    reader: IMFSourceReader,
    writer: IMFSinkWriter,
    layout: Layout,
    video_out: u32,
    /// Writer stream for the sound, if kept.
    audio_out: Option<u32>,
    duration: Duration,
}

/// Opens the reader and the writer.
///
/// SAFETY: Media Foundation must be started.
unsafe fn open(input: &Path, output: &Path, options: &TrimOptions) -> Result<Pipeline> {
    // SAFETY: Media Foundation calls with valid arguments.
    unsafe {
        let mut attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attributes, 2)?;
        let attributes = attributes.ok_or(RecordError::ThreadLost)?;
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(input.as_os_str()), &attributes)?;
        #[allow(clippy::cast_sign_loss)]
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(VIDEO, true)?;

        let nv12 = MFCreateMediaType()?;
        nv12.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        nv12.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        reader.SetCurrentMediaType(VIDEO, None, &nv12)?;
        let decoded = reader.GetCurrentMediaType(VIDEO)?;
        let layout = layout(&decoded)?;
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

        #[allow(clippy::cast_sign_loss)]
        let duration = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ok()
            .and_then(|v| u64::try_from(&v).ok())
            .map_or(Duration::ZERO, |t| Duration::from_nanos(t * 100));

        let mut writer_attributes: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut writer_attributes, 1)?;
        let writer_attributes = writer_attributes.ok_or(RecordError::ThreadLost)?;
        writer_attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        let writer = MFCreateSinkWriterFromURL(
            &HSTRING::from(output.as_os_str()),
            None,
            &writer_attributes,
        )?;
        #[allow(clippy::cast_sign_loss)]
        let settings = VideoSettings {
            width: layout.visible.width() as u32,
            height: layout.visible.height() as u32,
            fps,
            codec: VideoCodec::H264,
            quality: options.quality,
        };
        let encoded = video_type(&settings, &MFVideoFormat_H264)?;
        encoded.SetUINT32(&MF_MT_AVG_BITRATE, settings.bitrate())?;
        #[allow(clippy::cast_sign_loss)]
        encoded.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
        let video_out = writer.AddStream(&encoded)?;
        writer.SetInputMediaType(
            video_out,
            &video_type(&settings, &MFVideoFormat_NV12)?,
            None,
        )?;

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
                    Ok(stream)
                });
            match configured {
                Ok(stream) => Some(stream),
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
        Ok(Pipeline {
            reader,
            writer,
            layout,
            video_out,
            audio_out,
            duration,
        })
    }
}

/// Writes the part of `input` between `options.start` and `options.end` to
/// `output` (H.264 + AAC MP4). `progress` receives the fraction done and
/// returns `false` to cancel.
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

/// SAFETY: Media Foundation must be started.
unsafe fn run(
    input: &Path,
    output: &Path,
    options: &TrimOptions,
    progress: &mut impl FnMut(f32) -> bool,
) -> Result<TrimStats> {
    // SAFETY: Media Foundation calls with valid arguments.
    unsafe {
        let p = open(input, output, options)?;
        let start = ticks(options.start);
        let end = options
            .end
            .map_or(i64::MAX, ticks)
            .min(if p.duration.is_zero() {
                i64::MAX
            } else {
                ticks(p.duration)
            });
        if end <= start {
            return Err(RecordError::EmptyRegion);
        }
        if start > 0 {
            // Lands on the key frame before `start`; earlier frames are
            // decoded and dropped below.
            p.reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(start))?;
        }
        p.writer.BeginWriting()?;

        let span = (end.min(ticks(p.duration).max(start + 1)) - start).max(1);
        let mut frames = 0u64;
        let mut last_video = 0i64;
        let (mut video_done, mut audio_done) = (false, p.audio_out.is_none());
        #[allow(clippy::cast_sign_loss)]
        let any = MF_SOURCE_READER_ANY_STREAM.0 as u32;
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
            let finished = has(MF_SOURCE_READERF_ENDOFSTREAM.0) || time >= end;
            if finished {
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
            let duration = sample.GetSampleDuration().unwrap_or(0);
            if time + duration <= start {
                continue;
            }
            let at = (time - start).max(0);
            if is_video {
                let out = crop_nv12(&sample, p.layout)?;
                out.SetSampleTime(at)?;
                out.SetSampleDuration(duration)?;
                p.writer.WriteSample(p.video_out, &out)?;
                frames += 1;
                last_video = at + duration;
                #[allow(clippy::cast_precision_loss)]
                let done = (at as f64 / span as f64).min(1.0);
                #[allow(clippy::cast_possible_truncation)]
                if !progress(done as f32) {
                    return Err(RecordError::Cancelled);
                }
            } else if let Some(stream) = p.audio_out {
                sample.SetSampleTime(at)?;
                p.writer.WriteSample(stream, &sample)?;
            }
        }
        if frames == 0 {
            return Err(RecordError::EmptyRegion);
        }
        p.writer.Finalize()?;
        progress(1.0);
        #[allow(clippy::cast_sign_loss)]
        Ok(TrimStats {
            width: p.layout.visible.width() as u32,
            height: p.layout.visible.height() as u32,
            frames,
            duration: Duration::from_nanos(last_video as u64 * 100),
        })
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
