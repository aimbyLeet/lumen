//! Reference colour math for the Lumen pipeline.
//!
//! Everything here is plain, platform-independent Rust and serves two roles:
//! 1. the *specification* the GPU shaders must match, and
//! 2. the measuring stick for end-to-end quality tests (ΔE2000, PSNR).

pub mod lab;
pub mod metrics;
pub mod subsample;
pub mod ycbcr;
