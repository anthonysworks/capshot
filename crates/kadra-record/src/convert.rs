//! Colour conversion, cropping and scaling on the GPU's video processor.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;

use kadra_capture::Gpu;
use kadra_core::Rect;
use windows::Win32::Foundation::{RECT, TRUE};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
    D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoContext1,
    ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_TYPE,
    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
    DXGI_FORMAT, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, Result};

fn win_rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

pub struct Converter {
    device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    input_view: ID3D11VideoProcessorInputView,
    output_size: (u32, u32),
}

impl Converter {
    /// Converts the `crop` area of `input` to an NV12 frame of `output_size`.
    pub fn new(
        gpu: &Gpu,
        input: &ID3D11Texture2D,
        input_size: (u32, u32),
        crop: Rect,
        output_size: (u32, u32),
        fps: u32,
    ) -> Result<Self> {
        let rate = DXGI_RATIONAL {
            Numerator: fps,
            Denominator: 1,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: input_size.0,
            InputHeight: input_size.1,
            OutputFrameRate: rate,
            OutputWidth: output_size.0,
            OutputHeight: output_size.1,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };

        // SAFETY: COM calls on live interfaces with valid descriptions.
        unsafe {
            let video_device: ID3D11VideoDevice = gpu.device().cast()?;
            let video_context: ID3D11VideoContext = gpu.context().cast()?;
            let enumerator = video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;

            let view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: 0,
                    },
                },
            };
            let mut input_view = None;
            video_device.CreateVideoProcessorInputView(
                input,
                &enumerator,
                &view_desc,
                Some(&mut input_view),
            )?;

            video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            video_context.VideoProcessorSetStreamSourceRect(
                &processor,
                0,
                true,
                Some(&win_rect(crop)),
            );
            #[allow(clippy::cast_possible_wrap)]
            let full = Rect::new(0, 0, output_size.0 as i32, output_size.1 as i32);
            video_context.VideoProcessorSetStreamDestRect(
                &processor,
                0,
                true,
                Some(&win_rect(full)),
            );
            video_context.VideoProcessorSetOutputTargetRect(
                &processor,
                true,
                Some(&win_rect(full)),
            );

            // sRGB in, BT.709 limited range out: what players expect for HD.
            if let Ok(ctx1) = video_context.cast::<ID3D11VideoContext1>() {
                ctx1.VideoProcessorSetStreamColorSpace1(
                    &processor,
                    0,
                    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                );
                ctx1.VideoProcessorSetOutputColorSpace1(
                    &processor,
                    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                );
            }

            Ok(Self {
                device: gpu.device().clone(),
                video_device,
                video_context,
                enumerator,
                processor,
                input_view: input_view.ok_or_else(windows::core::Error::empty)?,
                output_size,
            })
        }
    }

    /// Converts the current content of the input texture into a new NV12
    /// texture. A fresh texture per frame lets the encoder keep earlier ones
    /// for as long as it needs.
    pub fn convert(&self) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.output_size.0,
            Height: self.output_size.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
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
        let output_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };

        // SAFETY: COM calls with valid descriptions; the stream's view
        // reference is released after the blit.
        unsafe {
            let mut texture = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?;
            let texture = texture.ok_or_else(windows::core::Error::empty)?;

            let mut output_view = None;
            self.video_device.CreateVideoProcessorOutputView(
                &texture,
                &self.enumerator,
                &output_desc,
                Some(&mut output_view),
            )?;
            let output_view = output_view.ok_or_else(windows::core::Error::empty)?;

            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: TRUE,
                pInputSurface: ManuallyDrop::new(Some(self.input_view.clone())),
                ..Default::default()
            };
            let result = self.video_context.VideoProcessorBlt(
                &self.processor,
                &output_view,
                0,
                std::slice::from_ref(&stream),
            );
            ManuallyDrop::drop(&mut stream.pInputSurface);
            result?;
            Ok(texture)
        }
    }
}

/// Colour spaces handled by [`Blitter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// 8-bit sRGB, full range (BGRA textures).
    Rgb,
    /// BT.709 YCbCr, limited range (HD video).
    Bt709,
    /// BT.601 YCbCr, limited range (older SD video).
    Bt601,
}

impl Space {
    fn dxgi(self) -> DXGI_COLOR_SPACE_TYPE {
        match self {
            Self::Rgb => DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
            Self::Bt709 => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            Self::Bt601 => DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P601,
        }
    }
}

/// A video processor blit: crops, scales and converts between YUV and RGB
/// on the GPU's video engine, from any texture to any texture.
pub struct Blitter {
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    /// Input views of decoder textures, by (texture address, array slice):
    /// decoders cycle through a small pool of textures.
    inputs: RefCell<HashMap<(usize, u32), ID3D11VideoProcessorInputView>>,
}

impl Blitter {
    pub fn new(
        gpu: &Gpu,
        input_size: (u32, u32),
        output_size: (u32, u32),
        fps: u32,
        (from, to): (Space, Space),
    ) -> Result<Self> {
        let rate = DXGI_RATIONAL {
            Numerator: fps.max(1),
            Denominator: 1,
        };
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: rate,
            InputWidth: input_size.0,
            InputHeight: input_size.1,
            OutputFrameRate: rate,
            OutputWidth: output_size.0,
            OutputHeight: output_size.1,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_QUALITY,
        };
        // SAFETY: COM calls on live interfaces with valid descriptions.
        unsafe {
            let video_device: ID3D11VideoDevice = gpu.device().cast()?;
            let video_context: ID3D11VideoContext = gpu.context().cast()?;
            let enumerator = video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = video_device.CreateVideoProcessor(&enumerator, 0)?;
            video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            // No driver "enhancements" (sharpening, denoising): screen
            // content must come out as it went in.
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            if let Ok(ctx1) = video_context.cast::<ID3D11VideoContext1>() {
                ctx1.VideoProcessorSetStreamColorSpace1(&processor, 0, from.dxgi());
                ctx1.VideoProcessorSetOutputColorSpace1(&processor, to.dxgi());
            }
            Ok(Self {
                video_device,
                video_context,
                enumerator,
                processor,
                inputs: RefCell::default(),
            })
        }
    }

    /// Input view of `slice` of `texture` (cached).
    pub fn input(
        &self,
        texture: &ID3D11Texture2D,
        slice: u32,
    ) -> Result<ID3D11VideoProcessorInputView> {
        let key = (texture.as_raw() as usize, slice);
        if let Some(view) = self.inputs.borrow().get(&key) {
            return Ok(view.clone());
        }
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: slice,
                },
            },
        };
        let mut view = None;
        // SAFETY: valid texture and description.
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                texture,
                &self.enumerator,
                &desc,
                Some(&mut view),
            )?;
        }
        let view = view.ok_or_else(windows::core::Error::empty)?;
        self.inputs.borrow_mut().insert(key, view.clone());
        Ok(view)
    }

    pub fn output(&self, texture: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorOutputView> {
        let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut view = None;
        // SAFETY: valid texture and description.
        unsafe {
            self.video_device.CreateVideoProcessorOutputView(
                texture,
                &self.enumerator,
                &desc,
                Some(&mut view),
            )?;
        }
        view.ok_or_else(windows::core::Error::empty)
    }

    /// Draws the `source` area of `input` into the `dest` area of `output`;
    /// pixels outside `dest` are left untouched.
    pub fn blit(
        &self,
        input: &ID3D11VideoProcessorInputView,
        source: Rect,
        output: &ID3D11VideoProcessorOutputView,
        dest: Rect,
    ) -> Result<()> {
        // SAFETY: live views; the stream's reference is released after the
        // blit.
        unsafe {
            let ctx = &self.video_context;
            let p = &self.processor;
            ctx.VideoProcessorSetStreamSourceRect(p, 0, true, Some(&win_rect(source)));
            ctx.VideoProcessorSetStreamDestRect(p, 0, true, Some(&win_rect(dest)));
            ctx.VideoProcessorSetOutputTargetRect(p, true, Some(&win_rect(dest)));
            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: TRUE,
                pInputSurface: ManuallyDrop::new(Some(input.clone())),
                ..Default::default()
            };
            let result = ctx.VideoProcessorBlt(p, output, 0, std::slice::from_ref(&stream));
            ManuallyDrop::drop(&mut stream.pInputSurface);
            result
        }
    }
}

/// A new texture the video processor and Direct2D can draw into.
pub fn render_texture(gpu: &Gpu, size: (u32, u32), format: DXGI_FORMAT) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size.0,
        Height: size.1,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
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
    let mut texture = None;
    // SAFETY: valid description and out pointer.
    unsafe {
        gpu.device()
            .CreateTexture2D(&desc, None, Some(&mut texture))?;
    }
    texture.ok_or_else(windows::core::Error::empty)
}
