//! Editing a video: decode the kept segments and encode them again, one
//! after the other, optionally faster or slower and cropped.
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
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_MPEG2_PROFILE,
    MF_MT_SUBTYPE, MF_PD_DURATION, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ANY_STREAM,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_AUDIO_STREAM,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READER_MEDIASOURCE,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MF_VERSION, MF2DBuffer_LockFlags_Read,
    MFAudioFormat_AAC, MFAudioFormat_PCM, MFCreateAttributes, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFCreateSourceReaderFromURL,
    MFMediaType_Audio, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown, MFStartup, MFVideoArea,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoFormat_RGB32, eAVEncH264VProfile_High,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING, Interface};

use crate::compose::{Composer, Composition, to_nv12};
use crate::cursor::CursorTrack;
use crate::decode::rgb32_frame;
use crate::encoder::{VideoSettings, video_type};
use crate::stretch::{click_sound, mix_at, stretch};
use crate::{RecordError, Result};

#[allow(clippy::cast_sign_loss)]
const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
#[allow(clippy::cast_sign_loss)]
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
/// AAC at 192 kbit/s.
const AAC_BYTES_PER_SECOND: u32 = 24_000;

#[derive(Debug, Clone)]
pub struct TrimOptions {
    /// Parts of the video to keep, in order (`Duration::MAX` as an end
    /// means "until the end").
    pub segments: Vec<(Duration, Duration)>,
    /// Keep the sound track (if there is one). Ignored unless `speed` is 1.
    pub audio: bool,
    /// 0 (best) to 51 (smallest); edited videos default to a high quality.
    pub quality: u8,
    /// Playback speed: 2.0 plays twice as fast.
    pub speed: f64,
    /// Area to keep, in pixels of the video; `None` keeps the whole frame.
    pub crop: Option<Rect>,
    /// Drawn on the frames (overlays, cursor, clicks, canvas).
    pub composition: Composition,
    /// Cursor recorded with the video (needed for cursor and click effects).
    pub track: Option<CursorTrack>,
}

impl TrimOptions {
    /// The whole video, unchanged.
    pub fn whole(audio: bool, quality: u8) -> Self {
        Self {
            segments: vec![(Duration::ZERO, Duration::MAX)],
            audio,
            quality,
            speed: 1.0,
            crop: None,
            composition: Composition::default(),
            track: None,
        }
    }

    fn keeps_sound(&self) -> bool {
        self.audio
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
    /// Sample rate and channels of the sound written.
    audio_format: (u32, u32),
    /// The source has no sound: the track is generated (click sounds).
    synth_audio: bool,
    /// Frames are decoded as RGB32 and composited.
    composer: Option<Composer>,
    duration: Duration,
}

/// Opens the reader and the writer.
///
/// SAFETY: Media Foundation must be started.
#[allow(clippy::too_many_lines)]
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

        // Composited frames are decoded as RGB32; plain cuts stay in NV12.
        let compose = !options.composition.is_empty();
        let wanted = MFCreateMediaType()?;
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        let subtype = if compose {
            &MFVideoFormat_RGB32
        } else {
            &MFVideoFormat_NV12
        };
        wanted.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        reader.SetCurrentMediaType(VIDEO, None, &wanted)?;
        let decoded = reader.GetCurrentMediaType(VIDEO)?;
        let mut layout = layout(&decoded)?;
        let cropped = apply_crop(layout.visible, options.crop);
        let composer = compose.then(|| {
            let visible = layout.visible;
            #[allow(clippy::cast_sign_loss)]
            let frame = (visible.width() as u32, visible.height() as u32);
            Composer::new(
                &options.composition,
                options.track.clone(),
                frame,
                Some(cropped.offset(-visible.left, -visible.top)),
            )
        });
        if composer.is_none() {
            layout.visible = cropped;
        }
        #[allow(clippy::cast_sign_loss)]
        let (out_w, out_h) = composer.as_ref().map_or(
            (cropped.width() as u32, cropped.height() as u32),
            Composer::output_size,
        );
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
        let settings = VideoSettings {
            width: out_w & !1,
            height: out_h & !1,
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

        let mut audio_format = (48_000, 2);
        let audio_out = if options.keeps_sound() && reader.SetStreamSelection(AUDIO, true).is_ok() {
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
            layout,
            video_out,
            audio_out,
            audio_format,
            synth_audio,
            composer,
            duration,
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

/// Segments clipped to the video and in 100 ns units, in order.
fn clip_segments(segments: &[(Duration, Duration)], duration: Duration) -> Vec<(i64, i64)> {
    let limit = if duration.is_zero() {
        i64::MAX
    } else {
        ticks(duration)
    };
    let mut out: Vec<(i64, i64)> = segments
        .iter()
        .map(|&(a, b)| (ticks(a).min(limit), ticks(b).min(limit)))
        .filter(|(a, b)| b > a)
        .collect();
    out.sort_unstable();
    out
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
        let speed = if options.speed > 0.0 {
            options.speed
        } else {
            1.0
        };
        let scale = |t: i64| (t as f64 / speed).round() as i64;
        let total: i64 = segments
            .iter()
            .map(|(a, b)| (*b).min(ticks(p.duration).max(*a + 1)) - a)
            .sum::<i64>()
            .max(1);
        p.writer.BeginWriting()?;

        #[allow(clippy::cast_sign_loss)]
        let any = MF_SOURCE_READER_ANY_STREAM.0 as u32;
        // The sound is rebuilt (time-stretched, clicks mixed in) rather
        // than copied when the speed changes or clicks are heard.
        let rebuild_audio = p.audio_out.is_some()
            && !p.synth_audio
            && ((speed - 1.0).abs() > 1e-6 || options.click_sound());
        let clicks: Vec<f64> = if options.click_sound() {
            options
                .track
                .as_ref()
                .map_or_else(Vec::new, |t| t.clicks.iter().map(|c| c.t).collect())
        } else {
            Vec::new()
        };
        let click = click_sound(p.audio_format.0);
        let (rate, channels) = (p.audio_format.0, p.audio_format.1 as usize);
        let mut frames = 0u64;
        // Output time where the current segment starts, and source time
        // already written (for progress).
        let mut offset = 0i64;
        let mut done_before = 0i64;
        let mut segment_starts = Vec::with_capacity(segments.len());
        for &(start, end) in &segments {
            segment_starts.push(offset);
            p.reader.SetStreamSelection(VIDEO, true)?;
            if p.audio_out.is_some() && !p.synth_audio {
                p.reader.SetStreamSelection(AUDIO, true)?;
            }
            // Lands on the key frame before `start`; earlier frames are
            // decoded and dropped below.
            p.reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(start))?;
            let (mut video_done, mut audio_done) = (false, p.audio_out.is_none() || p.synth_audio);
            let mut segment_end = 0i64;
            let mut pcm: Vec<i16> = Vec::new();
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
                let duration = sample.GetSampleDuration().unwrap_or(0);
                if time + duration <= start {
                    continue;
                }
                let at = offset + scale((time - start).max(0));
                if is_video {
                    let out = match &p.composer {
                        Some(composer) => {
                            let frame = rgb32_frame(&sample, p.layout.coded, p.layout.visible)?;
                            let rendered = composer.render(frame, time as f64 / 1e7);
                            nv12_sample(&to_nv12(&rendered))?
                        }
                        None => crop_nv12(&sample, p.layout)?,
                    };
                    out.SetSampleTime(at)?;
                    out.SetSampleDuration(scale(duration))?;
                    p.writer.WriteSample(p.video_out, &out)?;
                    frames += 1;
                    segment_end = segment_end.max(at + scale(duration));
                    let done = (done_before + (time - start).max(0)) as f64 / total as f64;
                    if !progress(done.min(1.0) as f32) {
                        return Err(RecordError::Cancelled);
                    }
                } else if rebuild_audio {
                    pcm.extend(read_pcm(&sample)?);
                } else if let Some(stream) = p.audio_out {
                    sample.SetSampleTime(at)?;
                    p.writer.WriteSample(stream, &sample)?;
                }
            }
            if rebuild_audio && let Some(stream) = p.audio_out {
                let mut out = stretch(&pcm, channels, rate, speed);
                let here: Vec<f64> = clicks
                    .iter()
                    .filter(|&&t| t >= start as f64 / 1e7 && t < end as f64 / 1e7)
                    .map(|&t| (t - start as f64 / 1e7) / speed)
                    .collect();
                mix_at(&mut out, channels, rate, &click, &here, 0.7);
                write_pcm(&p.writer, stream, &out, channels, rate, offset)?;
            }
            done_before += end.min(ticks(p.duration).max(start + 1)) - start;
            // The next segment follows the last frame written.
            offset = offset.max(segment_end);
        }
        if p.synth_audio
            && let Some(stream) = p.audio_out
        {
            let total_frames = (offset as f64 / 1e7 * f64::from(rate)) as usize;
            let mut out = vec![0i16; total_frames * channels];
            let times: Vec<f64> = clicks
                .iter()
                .filter_map(|&t| {
                    segments
                        .iter()
                        .zip(&segment_starts)
                        .find_map(|(&(a, b), &o)| {
                            let tt = (t * 1e7) as i64;
                            (tt >= a && tt < b).then(|| (o + scale(tt - a)) as f64 / 1e7)
                        })
                })
                .collect();
            mix_at(&mut out, channels, rate, &click, &times, 0.7);
            write_pcm(&p.writer, stream, &out, channels, rate, 0)?;
        }
        if frames == 0 {
            return Err(RecordError::EmptyRegion);
        }
        p.writer.Finalize()?;
        progress(1.0);
        #[allow(clippy::cast_sign_loss)]
        let (width, height) = p.composer.as_ref().map_or(
            (p.layout.visible.width() as u32, p.layout.visible.height() as u32),
            Composer::output_size,
        );
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

/// Writes interleaved PCM from output time `start` (100 ns units), in
/// chunks the encoder takes comfortably.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn write_pcm(
    writer: &IMFSinkWriter,
    stream: u32,
    pcm: &[i16],
    channels: usize,
    rate: u32,
    start: i64,
) -> Result<()> {
    const CHUNK: usize = 4096;
    let channels = channels.max(1);
    for (i, chunk) in pcm.chunks(CHUNK * channels).enumerate() {
        let frames = chunk.len() / channels;
        let time = start + (i * CHUNK) as i64 * 10_000_000 / i64::from(rate);
        let duration = frames as i64 * 10_000_000 / i64::from(rate);
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
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
            writer.WriteSample(stream, &sample)?;
        }
    }
    Ok(())
}

/// A packed NV12 frame as an encoder sample.
fn nv12_sample(nv12: &[u8]) -> Result<IMFSample> {
    // SAFETY: the buffer is locked while the frame is copied in.
    unsafe {
        #[allow(clippy::cast_possible_truncation)]
        let buffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
        let mut data = std::ptr::null_mut();
        buffer.Lock(&mut data, None, None)?;
        std::ptr::copy_nonoverlapping(nv12.as_ptr(), data, nv12.len());
        buffer.Unlock()?;
        #[allow(clippy::cast_possible_truncation)]
        buffer.SetCurrentLength(nv12.len() as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        Ok(sample)
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
        let clipped = clip_segments(
            &[(s(5.0), s(9.0)), (s(1.0), s(2.0)), (s(3.0), s(3.0))],
            s(8.0),
        );
        assert_eq!(
            clipped,
            vec![(10_000_000, 20_000_000), (50_000_000, 80_000_000)]
        );
        assert_eq!(
            clip_segments(&[(Duration::ZERO, Duration::MAX)], s(2.0)),
            vec![(0, 20_000_000)]
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
}
