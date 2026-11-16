//! Videos added after the edited one: each is scaled to fit the frame of
//! the result (black bars around it) and keeps its sound.

use std::path::Path;

use capshot_core::Rect;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::Win32::Media::MediaFoundation::{
    IMFSample, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR,
};

use crate::convert::{Blitter, Space, render_texture};
use crate::media::{Decoded, VIDEO, VideoFile};
use crate::sound::{PendingPcm, decode_music};
use crate::stretch::{Bed, mix_bed};
use crate::trim::{Pipeline, write_texture};
use crate::{RecordError, Result};

/// What the clips are written with: the timing the main video uses and the
/// sound format of the result.
pub(crate) struct Writing<'a> {
    pub frame_ticks: i64,
    pub min_gap: i64,
    pub rate: u32,
    pub channels: usize,
    pub music: Option<&'a (Vec<i16>, Bed)>,
}

/// Writes `clip` after the result written so far (`offset`, 100 ns units).
/// Returns where it ends and how many frames were written.
///
/// SAFETY: Media Foundation must be started; `p` is the open pipeline.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) unsafe fn append_clip(
    p: &Pipeline,
    clip: &Path,
    offset: i64,
    writing: &Writing<'_>,
    progress: &mut impl FnMut(f32) -> bool,
) -> Result<(i64, u64)> {
    let Writing {
        frame_ticks,
        min_gap,
        rate,
        channels,
        music,
    } = *writing;
    // SAFETY: Media Foundation and Direct3D calls with valid arguments.
    unsafe {
        let gpu = p.frames.gpu();
        let file = VideoFile::open(clip, &p.manager)?;
        let decoded = Decoded::new(gpu, file.layout.coded);

        // The clip, scaled into the frame of the result.
        let visible = file.layout.visible;
        let (cw, ch) = (visible.width() as u32, visible.height() as u32);
        let (ow, oh) = p.frames.size;
        let scale = (f64::from(ow) / f64::from(cw)).min(f64::from(oh) / f64::from(ch));
        let dw = ((f64::from(cw) * scale) as u32 & !1).clamp(2, ow);
        let dh = ((f64::from(ch) * scale) as u32 & !1).clamp(2, oh);
        let (dx, dy) = (((ow - dw) / 2) & !1, ((oh - dh) / 2) & !1);
        let dest = Rect::new(dx as i32, dy as i32, (dx + dw) as i32, (dy + dh) as i32);
        let scaler = Blitter::new(
            gpu,
            file.layout.coded,
            (ow, oh),
            file.fps,
            (file.space, Space::Bt709),
        )?;

        // A black frame (NV12: luma 16, chroma 128) to start each one from.
        let black = render_texture(gpu, (ow, oh), DXGI_FORMAT_NV12)?;
        let pixels = ow as usize * oh as usize;
        let mut nv12 = vec![16u8; pixels];
        nv12.resize(pixels + pixels / 2, 128);
        gpu.context()
            .UpdateSubresource(&black, 0, None, nv12.as_ptr().cast(), ow, 0);

        // Its sound; silent when it has none (or when the result has none).
        let length = ticks_of(file.duration);
        let mut pending = match p.audio_out {
            Some(stream) => {
                let mut pcm =
                    decode_music(clip, 0.0, f64::INFINITY, rate, channels).unwrap_or_default();
                let wanted = (length as f64 / 1e7 * f64::from(rate)) as usize * channels;
                pcm.resize(pcm.len().max(wanted), 0);
                if let Some((music, bed)) = music {
                    mix_bed(&mut pcm, channels, rate, offset as f64 / 1e7, music, bed);
                }
                Some(PendingPcm::new(pcm, stream, offset))
            }
            None => None,
        };

        let (mut first, mut last_written, mut end, mut written) = (None, i64::MIN, offset, 0u64);
        loop {
            let (mut flags, mut time) = (0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            file.reader.ReadSample(
                VIDEO,
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
            let first = *first.get_or_insert(time);
            let at = offset + (time - first).max(0);
            let span = sample.GetSampleDuration().unwrap_or(frame_ticks);
            end = end.max(at + span);
            if last_written == i64::MIN || at - last_written >= min_gap {
                let (texture, slice) = decoded.texture(&sample)?;
                let output = render_texture(gpu, (ow, oh), DXGI_FORMAT_NV12)?;
                gpu.context().CopyResource(&output, &black);
                scaler.blit(
                    &scaler.input(&texture, slice)?,
                    visible,
                    &scaler.output(&output)?,
                    dest,
                )?;
                drop(sample);
                if let Some(pending) = &mut pending {
                    pending.write_until(&p.writer, at + 5_000_000, channels, rate)?;
                }
                write_texture(&p.writer, p.video_out, &output, at, span)?;
                last_written = at;
                written += 1;
            }
            // Stays below 100 % until everything is written.
            if !progress(0.99) {
                return Err(RecordError::Cancelled);
            }
        }
        if let Some(pending) = &mut pending {
            pending.write_until(&p.writer, i64::MAX, channels, rate)?;
        }
        Ok((end, written))
    }
}

fn ticks_of(d: std::time::Duration) -> i64 {
    i64::try_from(d.as_nanos() / 100).unwrap_or(i64::MAX)
}
