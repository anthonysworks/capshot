//! Hardware video encoding and MP4 muxing with Media Foundation.
//!
//! The sink writer is given the Direct3D device, so NV12 textures go
//! straight from the video processor to the hardware encoder (NVENC, AMF,
//! Quick Sync) without ever being copied to system memory.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use kadra_capture::Gpu;
use kadra_core::VideoCodec;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFAttributes, IMFDXGIDeviceManager, IMFMediaType, IMFSinkWriter,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE,
    MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
    MF_SINK_WRITER_D3D_MANAGER, MF_VERSION, MFAudioFormat_AAC, MFAudioFormat_PCM,
    MFCreateAttributes, MFCreateDXGIDeviceManager, MFCreateDXGISurfaceBuffer, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Audio,
    MFMediaType_Video, MFNominalRange_16_235, MFSTARTUP_FULL, MFShutdown, MFStartup,
    MFVideoFormat_AV1, MFVideoFormat_H264, MFVideoFormat_HEVC, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, MFVideoPrimaries_BT709, MFVideoTransFunc_709,
    MFVideoTransferMatrix_BT709, eAVEncH264VProfile_High,
};
use windows::core::{GUID, HSTRING, Interface, Result};

/// Media Foundation time unit: 100 ns.
pub const TICKS_PER_SECOND: i64 = 10_000_000;

#[derive(Debug, Clone, Copy)]
pub struct VideoSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: VideoCodec,
    /// 0 (best) to 51 (smallest), like x264's CRF.
    pub quality: u8,
}

impl VideoSettings {
    /// Average bitrate for the requested quality.
    ///
    /// Hardware encoders are most reliable in VBR mode across vendors, so the
    /// CRF-like quality setting is mapped to a bitrate: 0.08 bit per pixel
    /// per frame at the default quality 23 for H.264 (≈10 Mbit/s at 1080p60),
    /// doubling every 6 steps, with the usual savings of newer codecs.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn bitrate(&self) -> u32 {
        let codec_factor = match self.codec {
            VideoCodec::H264 => 1.0,
            VideoCodec::Hevc => 0.65,
            VideoCodec::Av1 => 0.55,
        };
        let bpp = 0.08 * 2f64.powf((23.0 - f64::from(self.quality)) / 6.0) * codec_factor;
        let bits = f64::from(self.width) * f64::from(self.height) * f64::from(self.fps) * bpp;
        bits.clamp(1_000_000.0, 200_000_000.0) as u32
    }

    fn subtype(&self) -> GUID {
        match self.codec {
            VideoCodec::H264 => MFVideoFormat_H264,
            VideoCodec::Hevc => MFVideoFormat_HEVC,
            VideoCodec::Av1 => MFVideoFormat_AV1,
        }
    }
}

const fn pack(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

fn video_type(settings: &VideoSettings, subtype: &GUID) -> Result<IMFMediaType> {
    // SAFETY: attribute setters on a freshly created media type.
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        #[allow(clippy::cast_sign_loss)]
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(settings.width, settings.height))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, pack(settings.fps, 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        #[allow(clippy::cast_sign_loss)]
        {
            t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
            t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
            t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        }
        Ok(t)
    }
}

/// AAC bitrate: 192 kbit/s, transparent for screen recordings.
const AAC_BYTES_PER_SECOND: u32 = 24_000;

fn audio_type(subtype: &GUID) -> Result<IMFMediaType> {
    use crate::audio::{CHANNELS, SAMPLE_RATE};
    // SAFETY: attribute setters on a freshly created media type.
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
        #[allow(clippy::cast_possible_truncation)]
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS as u32)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        Ok(t)
    }
}

pub struct Encoder {
    writer: IMFSinkWriter,
    video: u32,
    audio: Option<u32>,
    /// Video and audio are written from different threads.
    lock: Mutex<()>,
    _manager: IMFDXGIDeviceManager,
}

// SAFETY: the sink writer lives in the multithreaded apartment and every
// call is serialised by `lock`.
unsafe impl Send for Encoder {}
// SAFETY: see above.
unsafe impl Sync for Encoder {}

impl Encoder {
    pub fn new(gpu: &Gpu, path: &Path, settings: &VideoSettings, audio: bool) -> Result<Self> {
        // SAFETY: Media Foundation calls with valid arguments; MFStartup is
        // reference counted and balanced in `Drop`.
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_FULL)?;

            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.ok_or_else(windows::core::Error::empty)?;
            manager.ResetDevice(gpu.device(), token)?;

            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 2)?;
            let attributes = attributes.ok_or_else(windows::core::Error::empty)?;
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            attributes.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager)?;

            let writer =
                MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attributes)?;

            let output = video_type(settings, &settings.subtype())?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, settings.bitrate())?;
            if settings.codec == VideoCodec::H264 {
                #[allow(clippy::cast_sign_loss)]
                output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            }
            let video = writer.AddStream(&output)?;
            let input = video_type(settings, &MFVideoFormat_NV12)?;
            writer.SetInputMediaType(video, &input, None)?;

            let audio = if audio {
                let output = audio_type(&MFAudioFormat_AAC)?;
                output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
                let stream = writer.AddStream(&output)?;
                let input = audio_type(&MFAudioFormat_PCM)?;
                #[allow(clippy::cast_possible_truncation)]
                let block = (crate::audio::CHANNELS * 2) as u32;
                input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)?;
                input.SetUINT32(
                    &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                    crate::audio::SAMPLE_RATE * block,
                )?;
                writer.SetInputMediaType(stream, &input, None)?;
                Some(stream)
            } else {
                None
            };
            writer.BeginWriting()?;

            Ok(Self {
                writer,
                video,
                audio,
                lock: Mutex::new(()),
                _manager: manager,
            })
        }
    }

    /// Queues one frame. `time` and `duration` are in 100 ns units.
    pub fn write_video(&self, texture: &ID3D11Texture2D, time: i64, duration: i64) -> Result<()> {
        // SAFETY: the texture is an NV12 surface of the encoder's device.
        unsafe {
            let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, texture, 0, false)?;
            let length = buffer.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
            buffer.SetCurrentLength(length)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
            let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            self.writer.WriteSample(self.video, &sample)
        }
    }

    /// Queues interleaved 16-bit PCM. Ignored if the file has no audio track.
    pub fn write_audio(&self, pcm: &[i16], time: i64, duration: i64) -> Result<()> {
        let Some(stream) = self.audio else {
            return Ok(());
        };
        if pcm.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::size_of_val(pcm);
        // SAFETY: the buffer is locked while `bytes` bytes are copied into it.
        unsafe {
            #[allow(clippy::cast_possible_truncation)]
            let buffer = MFCreateMemoryBuffer(bytes as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(pcm.as_ptr().cast::<u8>(), data, bytes);
            buffer.Unlock()?;
            #[allow(clippy::cast_possible_truncation)]
            buffer.SetCurrentLength(bytes as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(time)?;
            sample.SetSampleDuration(duration)?;
            let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            self.writer.WriteSample(stream, &sample)
        }
    }

    /// Flushes the encoder and writes the MP4 index.
    pub fn finish(&self) -> Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        // SAFETY: plain call; nothing is written afterwards.
        unsafe { self.writer.Finalize() }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: balances the MFStartup in `new`.
        let _ = unsafe { MFShutdown() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(codec: VideoCodec, quality: u8) -> VideoSettings {
        VideoSettings {
            width: 1920,
            height: 1080,
            fps: 60,
            codec,
            quality,
        }
    }

    #[test]
    fn bitrate_follows_quality_and_codec() {
        let default = settings(VideoCodec::H264, 23).bitrate();
        assert!((9_000_000..=11_000_000).contains(&default), "{default}");
        assert!(settings(VideoCodec::H264, 17).bitrate() > default * 19 / 10);
        assert!(settings(VideoCodec::Hevc, 23).bitrate() < default);
        assert!(settings(VideoCodec::Av1, 23).bitrate() < settings(VideoCodec::Hevc, 23).bitrate());
    }

    #[test]
    fn bitrate_is_clamped() {
        let tiny = VideoSettings {
            width: 16,
            height: 16,
            fps: 1,
            codec: VideoCodec::Av1,
            quality: 51,
        };
        assert_eq!(tiny.bitrate(), 1_000_000);
    }
}
