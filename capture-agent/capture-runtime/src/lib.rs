use std::{
    ffi::{CStr, CString, c_int},
    path::Path,
    ptr::{self, NonNull},
};

use anyhow::{Context, Result};
use capture_agent::{
    capture_agent_entry,
    ffmpeg::{VideoEncoder, install_encoder_factory},
    source::CapturedFrame,
    window_enumerator_entry,
};
use rusty_ffmpeg::ffi;

type FfmpegResult<T> = std::result::Result<T, String>;

struct Encoder {
    format: NonNull<ffi::AVFormatContext>,
    codec: NonNull<ffi::AVCodecContext>,
    stream: NonNull<ffi::AVStream>,
    scaler: NonNull<ffi::SwsContext>,
    frame: NonNull<ffi::AVFrame>,
    packet: NonNull<ffi::AVPacket>,
    frame_index: i64,
    source_width: c_int,
    source_height: c_int,
    encoded_width: c_int,
    encoded_height: c_int,
    padded_rgba: Vec<u8>,
}

struct OpeningResources {
    format: NonNull<ffi::AVFormatContext>,
    codec: Option<NonNull<ffi::AVCodecContext>>,
    scaler: Option<NonNull<ffi::SwsContext>>,
    frame: Option<NonNull<ffi::AVFrame>>,
    packet: Option<NonNull<ffi::AVPacket>>,
    committed: bool,
}

impl OpeningResources {
    fn new(format: NonNull<ffi::AVFormatContext>) -> Self {
        Self {
            format,
            codec: None,
            scaler: None,
            frame: None,
            packet: None,
            committed: false,
        }
    }
}

impl Drop for OpeningResources {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        unsafe {
            if let Some(packet) = self.packet {
                let mut raw = packet.as_ptr();
                ffi::av_packet_free(&mut raw);
            }
            if let Some(frame) = self.frame {
                let mut raw = frame.as_ptr();
                ffi::av_frame_free(&mut raw);
            }
            if let Some(scaler) = self.scaler {
                ffi::sws_freeContext(scaler.as_ptr());
            }
            if let Some(codec) = self.codec {
                let mut raw = codec.as_ptr();
                ffi::avcodec_free_context(&mut raw);
            }
            if !(*self.format.as_ptr()).pb.is_null() {
                let _ = ffi::avio_closep(&mut (*self.format.as_ptr()).pb);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn omni_capture_agent_main() -> i32 {
    guarded_entrypoint(|| {
        install_encoder_factory(create_encoder)?;
        Ok(capture_agent_entry::entrypoint())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn omni_window_enumerator_main() -> i32 {
    guarded_entrypoint(|| Ok(window_enumerator_entry::entrypoint()))
}

fn guarded_entrypoint(entrypoint: impl FnOnce() -> Result<i32>) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(entrypoint)) {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => {
            eprintln!("capture-runtime: {error:#}");
            70
        }
        Err(_) => {
            eprintln!("capture-runtime: internal panic");
            70
        }
    }
}

fn create_encoder(
    output: &Path,
    width: u32,
    height: u32,
    fps: u32,
    key_frame_interval: u32,
    bitrate_kbps: u32,
) -> Result<Box<dyn VideoEncoder>> {
    let encoder = Encoder::open(output, width, height, fps, key_frame_interval, bitrate_kbps)
        .map_err(anyhow::Error::msg)?;
    Ok(Box::new(encoder))
}

impl VideoEncoder for Encoder {
    fn write_frame(&mut self, frame: &CapturedFrame) -> Result<()> {
        unsafe { self.write_rgba(frame.rgba.as_ptr(), frame.rgba.len()) }
            .map_err(anyhow::Error::msg)
    }

    fn finish(mut self: Box<Self>) -> Result<()> {
        unsafe { Encoder::finish(&mut self) }.map_err(anyhow::Error::msg)
    }

    fn abort(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

impl Encoder {
    fn open(
        output: &Path,
        width: u32,
        height: u32,
        fps: u32,
        key_frame_interval: u32,
        bitrate_kbps: u32,
    ) -> FfmpegResult<Self> {
        if width == 0 || height == 0 || fps == 0 {
            return Err("invalid FFmpeg encoder options".to_string());
        }
        let output = output
            .to_str()
            .context("video output path is not valid UTF-8")
            .map_err(|error| error.to_string())?;
        let output = CString::new(output).map_err(|_| "video output path contains NUL")?;
        let format_name = c"matroska";
        let encoder_name = c"libopenh264";
        let mut format = ptr::null_mut();
        ffmpeg_result(
            unsafe {
                ffi::avformat_alloc_output_context2(
                    &mut format,
                    ptr::null(),
                    format_name.as_ptr(),
                    output.as_ptr(),
                )
            },
            "cannot allocate Matroska output context",
        )?;
        let format = NonNull::new(format).ok_or("FFmpeg returned a null output context")?;

        let result = unsafe {
            Self::open_inner(
                format,
                encoder_name,
                width,
                height,
                fps,
                bitrate_kbps,
                key_frame_interval,
            )
        };
        if result.is_err() {
            unsafe { ffi::avformat_free_context(format.as_ptr()) };
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn open_inner(
        format: NonNull<ffi::AVFormatContext>,
        encoder_name: &CStr,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_kbps: u32,
        key_frame_interval: u32,
    ) -> FfmpegResult<Self> {
        let mut resources = OpeningResources::new(format);
        let codec = NonNull::new(
            unsafe { ffi::avcodec_find_encoder_by_name(encoder_name.as_ptr()) }.cast_mut(),
        )
        .ok_or("bundled FFmpeg does not provide the libopenh264 encoder")?;
        let stream =
            NonNull::new(unsafe { ffi::avformat_new_stream(format.as_ptr(), ptr::null()) })
                .ok_or("cannot allocate FFmpeg video stream")?;
        let codec_context = NonNull::new(unsafe { ffi::avcodec_alloc_context3(codec.as_ptr()) })
            .ok_or("cannot allocate FFmpeg codec context")?;
        resources.codec = Some(codec_context);

        unsafe {
            let context = codec_context.as_ptr();
            (*context).codec_id = (*codec.as_ptr()).id;
            (*context).codec_type = ffi::AVMEDIA_TYPE_VIDEO;
            (*context).width = width.next_multiple_of(2) as c_int;
            (*context).height = height.next_multiple_of(2) as c_int;
            (*context).pix_fmt = ffi::AV_PIX_FMT_YUV420P;
            (*context).time_base = ffi::AVRational {
                num: 1,
                den: fps as c_int,
            };
            (*context).framerate = ffi::AVRational {
                num: fps as c_int,
                den: 1,
            };
            (*context).bit_rate = i64::from(bitrate_kbps) * 1_000;
            (*context).gop_size = key_frame_interval as c_int;
            (*context).max_b_frames = 0;
            if (*(*format.as_ptr()).oformat).flags & ffi::AVFMT_GLOBALHEADER as c_int != 0 {
                (*context).flags |= ffi::AV_CODEC_FLAG_GLOBAL_HEADER as c_int;
            }
        }

        let mut codec_options = ptr::null_mut();
        unsafe {
            ffi::av_dict_set(
                &mut codec_options,
                c"allow_skip_frames".as_ptr(),
                c"1".as_ptr(),
                0,
            );
            ffi::av_dict_set(
                &mut codec_options,
                c"rc_mode".as_ptr(),
                c"bitrate".as_ptr(),
                0,
            );
            ffi::av_dict_set(&mut codec_options, c"profile".as_ptr(), c"main".as_ptr(), 0);
        }
        let open_result = unsafe {
            ffi::avcodec_open2(codec_context.as_ptr(), codec.as_ptr(), &mut codec_options)
        };
        unsafe { ffi::av_dict_free(&mut codec_options) };
        ffmpeg_result(open_result, "cannot open libopenh264 encoder")?;

        unsafe {
            (*stream.as_ptr()).time_base = (*codec_context.as_ptr()).time_base;
        }
        ffmpeg_result(
            unsafe {
                ffi::avcodec_parameters_from_context(
                    (*stream.as_ptr()).codecpar,
                    codec_context.as_ptr(),
                )
            },
            "cannot copy FFmpeg codec parameters",
        )?;

        let open_io = unsafe {
            ffi::avio_open(
                &mut (*format.as_ptr()).pb,
                (*format.as_ptr()).url,
                ffi::AVIO_FLAG_WRITE as c_int,
            )
        };
        ffmpeg_result(open_io, "cannot open output video file")?;
        ffmpeg_result(
            unsafe { ffi::avformat_write_header(format.as_ptr(), ptr::null_mut()) },
            "cannot write Matroska header",
        )?;

        let encoded_width = unsafe { (*codec_context.as_ptr()).width };
        let encoded_height = unsafe { (*codec_context.as_ptr()).height };
        let scaler = NonNull::new(unsafe {
            ffi::sws_getContext(
                encoded_width,
                encoded_height,
                ffi::AV_PIX_FMT_RGBA,
                encoded_width,
                encoded_height,
                ffi::AV_PIX_FMT_YUV420P,
                ffi::SWS_BILINEAR as c_int,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        })
        .ok_or("cannot allocate FFmpeg pixel converter")?;
        resources.scaler = Some(scaler);
        let frame =
            NonNull::new(unsafe { ffi::av_frame_alloc() }).ok_or("cannot allocate FFmpeg frame")?;
        resources.frame = Some(frame);
        unsafe {
            (*frame.as_ptr()).format = ffi::AV_PIX_FMT_YUV420P as c_int;
            (*frame.as_ptr()).width = encoded_width;
            (*frame.as_ptr()).height = encoded_height;
        }
        ffmpeg_result(
            unsafe { ffi::av_frame_get_buffer(frame.as_ptr(), 32) },
            "cannot allocate FFmpeg frame buffer",
        )?;
        let packet = NonNull::new(unsafe { ffi::av_packet_alloc() })
            .ok_or("cannot allocate FFmpeg packet")?;
        resources.packet = Some(packet);

        resources.committed = true;

        Ok(Self {
            format,
            codec: codec_context,
            stream,
            scaler,
            frame,
            packet,
            frame_index: 0,
            source_width: width as c_int,
            source_height: height as c_int,
            encoded_width,
            encoded_height,
            padded_rgba: vec![0; encoded_width as usize * encoded_height as usize * 4],
        })
    }

    unsafe fn write_rgba(&mut self, rgba: *const u8, rgba_len: usize) -> Result<(), String> {
        let expected = self.source_width as usize * self.source_height as usize * 4;
        if rgba.is_null() || rgba_len != expected {
            return Err(format!(
                "invalid RGBA frame length {rgba_len}, expected {expected}"
            ));
        }
        ffmpeg_result(
            unsafe { ffi::av_frame_make_writable(self.frame.as_ptr()) },
            "FFmpeg frame is not writable",
        )?;
        let rgba = if self.source_width == self.encoded_width
            && self.source_height == self.encoded_height
        {
            rgba
        } else {
            unsafe { self.pad_rgba(rgba) };
            self.padded_rgba.as_ptr()
        };
        let source_data = [rgba, ptr::null(), ptr::null(), ptr::null()];
        let source_lines = [self.encoded_width * 4, 0, 0, 0];
        let destination_data = unsafe { (*self.frame.as_ptr()).data };
        let destination_lines = unsafe { (*self.frame.as_ptr()).linesize };
        let rows = unsafe {
            ffi::sws_scale(
                self.scaler.as_ptr(),
                source_data.as_ptr(),
                source_lines.as_ptr(),
                0,
                self.encoded_height,
                destination_data.as_ptr(),
                destination_lines.as_ptr(),
            )
        };
        if rows <= 0 {
            return Err("FFmpeg pixel conversion failed".to_string());
        }
        unsafe {
            (*self.frame.as_ptr()).pts = self.frame_index;
        }
        self.frame_index += 1;
        ffmpeg_result(
            unsafe { ffi::avcodec_send_frame(self.codec.as_ptr(), self.frame.as_ptr()) },
            "FFmpeg rejected a video frame",
        )?;
        unsafe { self.drain_packets(false) }
    }

    unsafe fn pad_rgba(&mut self, source: *const u8) {
        let source_stride = self.source_width as usize * 4;
        let target_stride = self.encoded_width as usize * 4;
        for row in 0..self.source_height as usize {
            let source_start = unsafe { source.add(row * source_stride) };
            let target_start = row * target_stride;
            unsafe {
                ptr::copy_nonoverlapping(
                    source_start,
                    self.padded_rgba.as_mut_ptr().add(target_start),
                    source_stride,
                );
            }
            if self.encoded_width > self.source_width {
                let last_pixel = target_start + source_stride - 4;
                self.padded_rgba
                    .copy_within(last_pixel..last_pixel + 4, target_start + source_stride);
            }
        }
        if self.encoded_height > self.source_height {
            let last_row = (self.source_height as usize - 1) * target_stride;
            let target_row = self.source_height as usize * target_stride;
            self.padded_rgba
                .copy_within(last_row..last_row + target_stride, target_row);
        }
    }

    unsafe fn finish(&mut self) -> Result<(), String> {
        ffmpeg_result(
            unsafe { ffi::avcodec_send_frame(self.codec.as_ptr(), ptr::null()) },
            "cannot flush FFmpeg encoder",
        )?;
        unsafe { self.drain_packets(true) }?;
        ffmpeg_result(
            unsafe { ffi::av_write_trailer(self.format.as_ptr()) },
            "cannot finalize Matroska video",
        )
    }

    unsafe fn drain_packets(&mut self, flushing: bool) -> Result<(), String> {
        loop {
            let code =
                unsafe { ffi::avcodec_receive_packet(self.codec.as_ptr(), self.packet.as_ptr()) };
            if code == ffi::AVERROR_EOF || code == -libc_errno_eagain() {
                if flushing && code != ffi::AVERROR_EOF {
                    return Err("FFmpeg did not finish flushing the encoder".to_string());
                }
                return Ok(());
            }
            ffmpeg_result(code, "cannot receive encoded FFmpeg packet")?;
            unsafe {
                ffi::av_packet_rescale_ts(
                    self.packet.as_ptr(),
                    (*self.codec.as_ptr()).time_base,
                    (*self.stream.as_ptr()).time_base,
                );
                (*self.packet.as_ptr()).stream_index = (*self.stream.as_ptr()).index;
            }
            let write_result = unsafe {
                ffi::av_interleaved_write_frame(self.format.as_ptr(), self.packet.as_ptr())
            };
            unsafe { ffi::av_packet_unref(self.packet.as_ptr()) };
            ffmpeg_result(write_result, "cannot write encoded FFmpeg packet")?;
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            let mut packet = self.packet.as_ptr();
            ffi::av_packet_free(&mut packet);
            let mut frame = self.frame.as_ptr();
            ffi::av_frame_free(&mut frame);
            ffi::sws_freeContext(self.scaler.as_ptr());
            let mut codec = self.codec.as_ptr();
            ffi::avcodec_free_context(&mut codec);
            if !(*self.format.as_ptr()).pb.is_null() {
                let _ = ffi::avio_closep(&mut (*self.format.as_ptr()).pb);
            }
            ffi::avformat_free_context(self.format.as_ptr());
        }
    }
}

fn ffmpeg_result(code: c_int, operation: &str) -> FfmpegResult<()> {
    if code >= 0 {
        return Ok(());
    }
    let mut buffer = [0_i8; 256];
    unsafe {
        ffi::av_strerror(code, buffer.as_mut_ptr(), buffer.len());
    }
    let detail = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_string_lossy();
    Err(format!("{operation}: {detail} ({code})"))
}

const fn libc_errno_eagain() -> c_int {
    11
}
