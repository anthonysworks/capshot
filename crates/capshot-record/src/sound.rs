//! Sound for edited videos: reading it as PCM, and writing prepared PCM to
//! the encoder in step with the video.

use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    IMFSample, IMFSinkWriter, IMFSourceReader, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR,
    MFAudioFormat_PCM, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSourceReaderFromURL, MFMediaType_Audio,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING};

use crate::Result;
use crate::media::AUDIO;
use crate::stretch::remix;

pub(crate) fn read_pcm(sample: &IMFSample) -> Result<Vec<i16>> {
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
pub(crate) struct AudioReader {
    reader: IMFSourceReader,
}

impl AudioReader {
    pub(crate) fn open(input: &Path) -> Result<Self> {
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
    pub(crate) fn read(
        &self,
        start: i64,
        end: i64,
        rate: u32,
        channels: usize,
    ) -> Result<Vec<i16>> {
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

/// Decodes a music file from `from` seconds on, for at most `seconds`, as
/// 16-bit PCM with `channels` channels at `rate`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn decode_music(
    path: &Path,
    from: f64,
    seconds: f64,
    rate: u32,
    channels: usize,
) -> Result<Vec<i16>> {
    // SAFETY: Media Foundation calls with valid arguments.
    unsafe {
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(AUDIO, true)?;
        let pcm = MFCreateMediaType()?;
        pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        reader.SetCurrentMediaType(AUDIO, None, &pcm)?;
        let current = reader.GetCurrentMediaType(AUDIO)?;
        let source_rate = current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
        let source_channels = current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)? as usize;
        let start = (from.max(0.0) * 1e7) as i64;
        if start > 0 {
            reader.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(start))?;
        }
        let wanted = if seconds.is_finite() {
            (seconds * f64::from(source_rate)) as usize * source_channels
        } else {
            usize::MAX
        };
        let mut out: Vec<i16> = Vec::new();
        while out.len() < wanted {
            let (mut flags, mut time) = (0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            reader.ReadSample(
                AUDIO,
                0,
                None,
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )?;
            if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 | MF_SOURCE_READERF_ERROR.0) as u32 != 0 {
                break;
            }
            let Some(sample) = sample else {
                continue;
            };
            let samples = read_pcm(&sample)?;
            // Seeking lands before `start`: skip what precedes it.
            let skip = if time < start {
                ((start - time) as f64 / 1e7 * f64::from(source_rate)) as usize * source_channels
            } else {
                0
            };
            out.extend_from_slice(samples.get(skip..).unwrap_or_default());
        }
        out.truncate(wanted.min(out.len()));
        Ok(remix(&out, source_channels, source_rate, channels, rate))
    }
}

/// Sound prepared for a segment, written in step with the video.
pub(crate) struct PendingPcm {
    pub(crate) pcm: Vec<i16>,
    stream: u32,
    /// Output time of the first sample (100 ns units).
    start: i64,
    /// Samples (per channel) already written.
    written: usize,
}

impl PendingPcm {
    pub(crate) fn new(pcm: Vec<i16>, stream: u32, start: i64) -> Self {
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
    pub(crate) fn write_until(
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
