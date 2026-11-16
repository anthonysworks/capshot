//! Reading video files through Media Foundation, shared by the export of an
//! edit ([`crate::trim`]), the videos added after it and the videos drawn
//! over it ([`crate::layers`]).

use std::cell::RefCell;
use std::path::Path;
use std::time::Duration;

use capshot_capture::Gpu;
use capshot_core::Rect;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer2, IMFAttributes, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFMediaType, IMFSample,
    IMFSourceReader, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE,
    MF_MT_MINIMUM_DISPLAY_APERTURE, MF_MT_SUBTYPE, MF_MT_YUV_MATRIX, MF_PD_DURATION,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_D3D_MANAGER, MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM,
    MF_SOURCE_READER_MEDIASOURCE, MF2DBuffer_LockFlags_Read, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFVideoArea, MFVideoFormat_NV12,
    MFVideoTransferMatrix_BT601,
};
use windows::core::{HSTRING, Interface};

use crate::convert::{Space, render_texture};
use crate::{RecordError, Result};

#[allow(clippy::cast_sign_loss)]
pub(crate) const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
#[allow(clippy::cast_sign_loss)]
pub(crate) const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

/// Decoded NV12 frame layout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Layout {
    pub coded: (u32, u32),
    pub visible: Rect,
}

pub(crate) fn layout(media_type: &IMFMediaType) -> Result<Layout> {
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

/// A video file opened for decoding to NV12 (on the GPU where the decoder
/// can).
pub(crate) struct VideoFile {
    pub reader: IMFSourceReader,
    pub layout: Layout,
    pub space: Space,
    /// Frames per second, rounded.
    pub fps: u32,
    pub duration: Duration,
}

impl VideoFile {
    /// Opens `path` with only its video stream selected.
    ///
    /// SAFETY: Media Foundation must be started.
    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    pub(crate) unsafe fn open(path: &Path, manager: &IMFDXGIDeviceManager) -> Result<Self> {
        // SAFETY: Media Foundation calls with valid arguments.
        unsafe {
            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 3)?;
            let attributes = attributes.ok_or(RecordError::ThreadLost)?;
            attributes.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, manager)?;
            attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
            attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            let reader =
                MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes)?;
            #[allow(clippy::cast_sign_loss)]
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(VIDEO, true)?;

            let wanted = MFCreateMediaType()?;
            wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            reader.SetCurrentMediaType(VIDEO, None, &wanted)?;
            let decoded = reader.GetCurrentMediaType(VIDEO)?;
            let layout = layout(&decoded)?;
            #[allow(clippy::cast_sign_loss)]
            let space = match decoded.GetUINT32(&MF_MT_YUV_MATRIX) {
                Ok(m) if m == MFVideoTransferMatrix_BT601.0 as u32 => Space::Bt601,
                _ => Space::Bt709,
            };
            let fps = decoded
                .GetUINT64(&MF_MT_FRAME_RATE)
                .ok()
                .map(|r| {
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
            Ok(Self {
                reader,
                layout,
                space,
                fps,
                duration,
            })
        }
    }
}

/// Decoded frames as textures: the decoder's own, or one uploaded from
/// system memory for decoders that only work there.
pub(crate) struct Decoded {
    gpu: Gpu,
    coded: (u32, u32),
    upload: RefCell<Option<ID3D11Texture2D>>,
}

impl Decoded {
    pub(crate) fn new(gpu: &Gpu, coded: (u32, u32)) -> Self {
        Self {
            gpu: gpu.clone(),
            coded,
            upload: RefCell::new(None),
        }
    }

    /// The texture of `sample` and the slice of it holding the frame.
    pub(crate) fn texture(&self, sample: &IMFSample) -> Result<(ID3D11Texture2D, u32)> {
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
            *upload = Some(render_texture(&self.gpu, self.coded, DXGI_FORMAT_NV12)?);
        }
        let texture = upload.clone().ok_or(RecordError::ThreadLost)?;
        let nv12 = packed_nv12(sample, self.coded)?;
        // SAFETY: `nv12` holds the whole frame at a pitch of the width.
        unsafe {
            self.gpu.context().UpdateSubresource(
                &texture,
                0,
                None,
                nv12.as_ptr().cast(),
                self.coded.0,
                0,
            );
        }
        Ok((texture, 0))
    }
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
