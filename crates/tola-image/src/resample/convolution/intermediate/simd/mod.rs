//! The intermediate pass's vector kernels, behind one interface.
//!
//! Each operation below has one implementation per target architecture, and a caller names the
//! operation rather than the architecture: no `cfg` reaches the convolution code that uses it.
//!
//! An operation reports what its kernels covered; taps it left uncovered fall to the shared scalar
//! remainder.

#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
mod scalar;
#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(target_arch = "aarch64")]
pub(in crate::resample::convolution::intermediate) use aarch64::{
    accumulate_pairs, accumulate_pairs_four, accumulate_tile,
};
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub(in crate::resample::convolution::intermediate) use scalar::{
    accumulate_pairs, accumulate_pairs_four, accumulate_tile,
};
#[cfg(target_arch = "x86_64")]
pub(in crate::resample::convolution::intermediate) use x86_64::{
    accumulate_pairs, accumulate_pairs_four, accumulate_tile,
};
