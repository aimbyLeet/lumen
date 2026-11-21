//! The libopus C API that Lumen uses (libopus/include/opus.h, opus_defines.h).
//! The library is built from libopus/ (1.5.2, xiph.org) by build.rs; safe
//! use is in `audio::opus`.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int};

#[repr(C)]
pub struct OpusEncoder {
    _private: [u8; 0],
}

#[repr(C)]
pub struct OpusDecoder {
    _private: [u8; 0],
}

pub const OPUS_OK: c_int = 0;
pub const OPUS_APPLICATION_AUDIO: c_int = 2049;
/// CELT only: the lowest delay (2.5 ms of look-ahead).
pub const OPUS_APPLICATION_RESTRICTED_LOWDELAY: c_int = 2051;
pub const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
pub const OPUS_SET_VBR_REQUEST: c_int = 4006;
pub const OPUS_SET_COMPLEXITY_REQUEST: c_int = 4010;
pub const OPUS_SET_PACKET_LOSS_PERC_REQUEST: c_int = 4014;
pub const OPUS_SET_SIGNAL_REQUEST: c_int = 4024;
pub const OPUS_GET_LOOKAHEAD_REQUEST: c_int = 4027;
pub const OPUS_RESET_STATE: c_int = 4028;
pub const OPUS_SIGNAL_MUSIC: c_int = 3002;

unsafe extern "C" {
    pub fn opus_get_version_string() -> *const c_char;
    pub fn opus_strerror(error: c_int) -> *const c_char;

    pub fn opus_encoder_create(
        fs: i32,
        channels: c_int,
        application: c_int,
        error: *mut c_int,
    ) -> *mut OpusEncoder;
    pub fn opus_encode_float(
        st: *mut OpusEncoder,
        pcm: *const f32,
        frame_size: c_int,
        data: *mut u8,
        max_data_bytes: i32,
    ) -> i32;
    pub fn opus_encoder_ctl(st: *mut OpusEncoder, request: c_int, ...) -> c_int;
    pub fn opus_encoder_destroy(st: *mut OpusEncoder);

    pub fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut OpusDecoder;
    /// `data` null: the frame was lost (packet loss concealment).
    pub fn opus_decode_float(
        st: *mut OpusDecoder,
        data: *const u8,
        len: i32,
        pcm: *mut f32,
        frame_size: c_int,
        decode_fec: c_int,
    ) -> c_int;
    pub fn opus_decoder_ctl(st: *mut OpusDecoder, request: c_int, ...) -> c_int;
    pub fn opus_decoder_destroy(st: *mut OpusDecoder);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_library_is_linked() {
        let v = unsafe { std::ffi::CStr::from_ptr(super::opus_get_version_string()) };
        assert_eq!(v.to_str().unwrap(), "libopus 1.5.2");
    }
}
