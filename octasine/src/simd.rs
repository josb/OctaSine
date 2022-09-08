//! SIMD abstraction

#[cfg(all(feature = "simd", target_arch = "x86_64"))]
use core::arch::x86_64::*;
use std::{
    marker::PhantomData,
    ops::{Add, AddAssign, Mul, Sub},
};

pub trait FallbackSine: Copy {
    fn sin(a: [f64; 2]) -> [f64; 2];
}

#[derive(Clone, Copy)]
pub struct FallbackSineStd;

impl FallbackSine for FallbackSineStd {
    fn sin([a1, a2]: [f64; 2]) -> [f64; 2] {
        [a1.sin(), a2.sin()]
    }
}

#[cfg(feature = "simd")]
#[derive(Clone, Copy)]
pub struct FallbackSineSleef;

#[cfg(feature = "simd")]
impl FallbackSine for FallbackSineSleef {
    fn sin([a1, a2]: [f64; 2]) -> [f64; 2] {
        unsafe {
            [
                sleef_sys::Sleef_cinz_sind1_u35purec(a1),
                sleef_sys::Sleef_cinz_sind1_u35purec(a2),
            ]
        }
    }
}

pub type FallbackPackedDoubleStd = FallbackPackedDouble<FallbackSineStd>;
#[cfg(feature = "simd")]
pub type FallbackPackedDoubleSleef = FallbackPackedDouble<FallbackSineSleef>;

pub trait SimdPackedDouble: Copy {
    const SAMPLES: usize;

    type Arr;

    unsafe fn new(value: f64) -> Self;
    unsafe fn new_zeroed() -> Self;
    unsafe fn new_from_pair(l: f64, r: f64) -> Self;
    unsafe fn load_ptr(source: *const f64) -> Self;
    unsafe fn from_arr(arr: Self::Arr) -> Self;
    unsafe fn to_arr(&self) -> Self::Arr;
    unsafe fn min(&self, other: Self) -> Self;
    unsafe fn max(&self, other: Self) -> Self;
    unsafe fn fast_sin(&self) -> Self;
    unsafe fn pairwise_horizontal_sum(&self) -> Self;
    unsafe fn interleave(&self, other: Self) -> Self;
    unsafe fn any_over_zero(&self) -> bool;
}

#[derive(Clone, Copy)]
pub struct FallbackPackedDouble<T>([f64; 2], PhantomData<T>);

impl<T: FallbackSine> SimdPackedDouble for FallbackPackedDouble<T> {
    const SAMPLES: usize = 1;

    type Arr = [f64; 2];

    #[inline]
    unsafe fn new(value: f64) -> Self {
        Self([value, value], Default::default())
    }
    #[inline]
    unsafe fn new_zeroed() -> Self {
        Self([0.0, 0.0], Default::default())
    }
    #[inline]
    unsafe fn new_from_pair(l: f64, r: f64) -> Self {
        Self([l, r], Default::default())
    }
    #[inline]
    unsafe fn from_arr(arr: Self::Arr) -> Self {
        Self(arr, Default::default())
    }
    #[inline]
    unsafe fn load_ptr(source: *const f64) -> Self {
        Self(*(source as *const [f64; 2]), Default::default())
    }
    #[inline]
    unsafe fn to_arr(&self) -> Self::Arr {
        self.0
    }
    #[inline]
    unsafe fn min(&self, other: Self) -> Self {
        let [a1, a2] = self.0;
        let [b1, b2] = other.0;

        Self([a1.min(b1), a2.min(b2)], self.1)
    }
    #[inline]
    unsafe fn max(&self, other: Self) -> Self {
        let [a1, a2] = self.0;
        let [b1, b2] = other.0;

        Self([a1.max(b1), a2.max(b2)], self.1)
    }
    #[inline]
    unsafe fn fast_sin(&self) -> Self {
        Self(T::sin(self.0), self.1)
    }
    #[inline]
    unsafe fn pairwise_horizontal_sum(&self) -> Self {
        let [l, r] = self.0;

        Self([l + r, l + r], self.1)
    }
    #[inline]
    unsafe fn interleave(&self, other: Self) -> Self {
        Self([self.0[0], other.0[1]], self.1)
    }
    #[inline]
    unsafe fn any_over_zero(&self) -> bool {
        (self.0[0] > 0.0) | (self.0[1] > 0.0)
    }
}

impl<T> Add for FallbackPackedDouble<T> {
    type Output = Self;

    #[inline(always)]
    fn add(self, rhs: Self) -> Self::Output {
        let [a1, a2] = self.0;
        let [b1, b2] = rhs.0;

        Self([a1 + b1, a2 + b2], self.1)
    }
}

impl<T> AddAssign for FallbackPackedDouble<T>
where
    FallbackPackedDouble<T>: Copy,
{
    #[inline(always)]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl<T> Sub for FallbackPackedDouble<T> {
    type Output = Self;

    #[inline(always)]
    fn sub(self, rhs: Self) -> Self::Output {
        let [a1, a2] = self.0;
        let [b1, b2] = rhs.0;

        Self([a1 - b1, a2 - b2], self.1)
    }
}

impl<T> Mul for FallbackPackedDouble<T> {
    type Output = Self;

    #[inline(always)]
    fn mul(self, rhs: Self) -> Self::Output {
        let [a1, a2] = self.0;
        let [b1, b2] = rhs.0;

        Self([a1 * b1, a2 * b2], self.1)
    }
}

#[cfg(feature = "simd")]
#[derive(Clone, Copy)]
pub struct AvxPackedDouble(__m256d);

#[cfg(feature = "simd")]
impl SimdPackedDouble for AvxPackedDouble {
    const SAMPLES: usize = 2;

    type Arr = [f64; 4];

    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn new(value: f64) -> Self {
        Self(_mm256_set1_pd(value))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn new_zeroed() -> Self {
        Self(_mm256_setzero_pd())
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn new_from_pair(l: f64, r: f64) -> Self {
        let lr = [l, r, l, r];

        Self(_mm256_loadu_pd(lr.as_ptr()))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn from_arr(arr: Self::Arr) -> Self {
        Self(_mm256_loadu_pd(arr.as_ptr()))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn load_ptr(source: *const f64) -> Self {
        Self(_mm256_loadu_pd(source))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn to_arr(&self) -> Self::Arr {
        let mut arr = Self::Arr::default();

        _mm256_storeu_pd(arr.as_mut_ptr(), self.0);

        arr
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn min(&self, other: Self) -> Self {
        Self(_mm256_min_pd(self.0, other.0))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn max(&self, other: Self) -> Self {
        Self(_mm256_max_pd(self.0, other.0))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn fast_sin(&self) -> Self {
        Self(sleef_sys::Sleef_cinz_sind4_u35avx(self.0))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn pairwise_horizontal_sum(&self) -> Self {
        Self(_mm256_add_pd(self.0, _mm256_permute_pd(self.0, 0b0101)))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn interleave(&self, other: Self) -> Self {
        Self(_mm256_blend_pd(self.0, other.0, 0b1010))
    }
    #[target_feature(enable = "avx")]
    #[inline]
    unsafe fn any_over_zero(&self) -> bool {
        _mm256_movemask_pd(_mm256_cmp_pd::<{ _CMP_GT_OQ }>(self.0, _mm256_setzero_pd())) != 0
    }
}

#[cfg(feature = "simd")]
impl Add for AvxPackedDouble {
    type Output = Self;

    #[inline(always)]
    fn add(self, rhs: Self) -> Self::Output {
        unsafe { Self(_mm256_add_pd(self.0, rhs.0)) }
    }
}

#[cfg(feature = "simd")]
impl AddAssign for AvxPackedDouble {
    #[inline(always)]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

#[cfg(feature = "simd")]
impl Sub for AvxPackedDouble {
    type Output = Self;

    #[inline(always)]
    fn sub(self, rhs: Self) -> Self::Output {
        unsafe { Self(_mm256_sub_pd(self.0, rhs.0)) }
    }
}

#[cfg(feature = "simd")]
impl Mul for AvxPackedDouble {
    type Output = Self;

    #[inline(always)]
    fn mul(self, rhs: Self) -> Self::Output {
        unsafe { Self(_mm256_mul_pd(self.0, rhs.0)) }
    }
}
