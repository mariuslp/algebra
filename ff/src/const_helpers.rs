use ark_serialize::{Read, Write};
use ark_std::ops::{Index, IndexMut};

use crate::BigInt;

/// A helper macro for emulating `for` loops in a `const` context.
/// # Usage
/// ```rust
/// # use ark_ff::const_for;
/// const fn for_in_const() {
///     let mut array = [0usize; 4];
///     const_for!((i in 0..(array.len())) { // We need to wrap the `array.len()` in parenthesis.
///         array[i] = i;
///     });
///     assert!(array[0] == 0);
///     assert!(array[1] == 1);
///     assert!(array[2] == 2);
///     assert!(array[3] == 3);
/// }
/// ```
#[macro_export]
macro_rules! const_for {
    (($i:ident in $start:tt..$end:tt)  $code:expr ) => {{
        let mut $i = $start;
        while $i < $end {
            $code
            $i += 1;
        }
    }};
}

/// A buffer to hold values of size 2 * N. This is mostly
/// a hack that's necessary until `generic_const_exprs` is stable.
#[derive(Copy, Clone)]
#[repr(C, align(8))]
pub(super) struct MulBuffer<const N: usize> {
    pub(super) b0: [u64; N],
    pub(super) b1: [u64; N],
}

impl<const N: usize> MulBuffer<N> {
    const fn new(b0: [u64; N], b1: [u64; N]) -> Self {
        Self { b0, b1 }
    }

    pub(super) const fn zeroed() -> Self {
        let b = [0u64; N];
        Self::new(b, b)
    }

    #[inline(always)]
    pub(super) const fn get(&self, index: usize) -> &u64 {
        if index < N {
            &self.b0[index]
        } else {
            &self.b1[index - N]
        }
    }

    #[inline(always)]
    pub(super) fn get_mut(&mut self, index: usize) -> &mut u64 {
        if index < N {
            &mut self.b0[index]
        } else {
            &mut self.b1[index - N]
        }
    }
}

impl<const N: usize> Index<usize> for MulBuffer<N> {
    type Output = u64;
    #[inline(always)]
    fn index(&self, index: usize) -> &Self::Output {
        self.get(index)
    }
}

impl<const N: usize> IndexMut<usize> for MulBuffer<N> {
    #[inline(always)]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.get_mut(index)
    }
}

/// A buffer to hold values of size 8 * N + 1 bytes. This is mostly
/// a hack that's necessary until `generic_const_exprs` is stable.
#[derive(Copy, Clone)]
#[repr(C, align(1))]
pub(super) struct SerBuffer<const N: usize> {
    pub(super) buffers: [[u8; 8]; N],
    pub(super) last: u8,
}

impl<const N: usize> SerBuffer<N> {
    pub(super) const fn zeroed() -> Self {
        Self {
            buffers: [[0u8; 8]; N],
            last: 0u8,
        }
    }

    #[inline(always)]
    pub(super) const fn get(&self, index: usize) -> &u8 {
        if index == 8 * N {
            &self.last
        } else {
            let part = index / 8;
            let in_buffer_index = index % 8;
            &self.buffers[part][in_buffer_index]
        }
    }

    #[inline(always)]
    pub(super) fn get_mut(&mut self, index: usize) -> &mut u8 {
        if index == 8 * N {
            &mut self.last
        } else {
            let part = index / 8;
            let in_buffer_index = index % 8;
            &mut self.buffers[part][in_buffer_index]
        }
    }

    #[allow(unsafe_code)]
    pub(super) const fn as_slice(&self) -> &[u8] {
        unsafe { ark_std::slice::from_raw_parts((self as *const Self) as *const u8, 8 * N + 1) }
    }

    #[inline(always)]
    pub(super) fn last_n_plus_1_bytes_mut(&mut self) -> impl Iterator<Item = &mut u8> {
        self.buffers[N - 1]
            .iter_mut()
            .chain(ark_std::iter::once(&mut self.last))
    }

    #[inline(always)]
    pub(super) fn copy_from_u8_slice(&mut self, other: &[u8]) {
        other.chunks(8).enumerate().for_each(|(i, chunk)| {
            if i < N {
                self.buffers[i][..chunk.len()].copy_from_slice(chunk);
            } else {
                self.last = chunk[0]
            }
        });
    }

    #[inline(always)]
    pub(super) fn copy_from_u64_slice(&mut self, other: &[u64; N]) {
        other
            .iter()
            .zip(&mut self.buffers)
            .for_each(|(other, this)| *this = other.to_le_bytes());
    }

    #[inline(always)]
    pub(super) fn to_bigint(self) -> BigInt<N> {
        let mut self_integer = BigInt::from(0u64);
        self_integer
            .0
            .iter_mut()
            .zip(self.buffers)
            .for_each(|(other, this)| *other = u64::from_le_bytes(this));
        self_integer
    }

    #[inline(always)]
    /// Write up to `num_bytes` bytes from `self` to `other`.
    /// `num_bytes` is allowed to range from `8 * (N - 1) + 1` to `8 * N + 1`.
    pub(super) fn write_up_to(
        &self,
        mut other: impl Write,
        num_bytes: usize,
    ) -> ark_std::io::Result<()> {
        debug_assert!(num_bytes <= 8 * N + 1, "index too large");
        debug_assert!(num_bytes > 8 * (N - 1), "index too small");
        // unconditionally write first `N - 1` limbs.
        for i in 0..(N - 1) {
            other.write_all(&self.buffers[i])?;
        }
        // for the `N`-th limb, depending on `index`, we can write anywhere from
        // 1 to all bytes.
        let remaining_bytes = num_bytes - (8 * (N - 1));
        let write_last_byte = remaining_bytes > 8;
        let num_last_limb_bytes = ark_std::cmp::min(8, remaining_bytes);
        other.write_all(&self.buffers[N - 1][..num_last_limb_bytes])?;
        if write_last_byte {
            other.write_all(&[self.last])?;
        }
        Ok(())
    }

    #[inline(always)]
    /// Read up to `num_bytes` bytes from `other` to `self`.
    /// `num_bytes` is allowed to range from `8 * (N - 1)` to `8 * N + 1`.
    pub(super) fn read_exact_up_to(
        &mut self,
        mut other: impl Read,
        num_bytes: usize,
    ) -> ark_std::io::Result<()> {
        debug_assert!(num_bytes <= 8 * N + 1, "index too large");
        debug_assert!(num_bytes > 8 * (N - 1), "index too small");
        // unconditionally write first `N - 1` limbs.
        for i in 0..(N - 1) {
            other.read_exact(&mut self.buffers[i])?;
        }
        // for the `N`-th limb, depending on `index`, we can write anywhere from
        // 1 to all bytes.
        let remaining_bytes = num_bytes - (8 * (N - 1));
        let write_last_byte = remaining_bytes > 8;
        let num_last_limb_bytes = ark_std::cmp::min(8, remaining_bytes);
        other.read_exact(&mut self.buffers[N - 1][..num_last_limb_bytes])?;
        if write_last_byte {
            let mut last = [0u8; 1];
            other.read_exact(&mut last)?;
            self.last = last[0];
        }
        Ok(())
    }
}

impl<const N: usize> Index<usize> for SerBuffer<N> {
    type Output = u8;
    #[inline(always)]
    fn index(&self, index: usize) -> &Self::Output {
        self.get(index)
    }
}

impl<const N: usize> IndexMut<usize> for SerBuffer<N> {
    #[inline(always)]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        self.get_mut(index)
    }
}

pub(super) struct RBuffer<const N: usize>(pub [u64; N], pub u64);

impl<const N: usize> RBuffer<N> {
    /// Find the number of bits in the binary decomposition of `self`.
    pub(super) const fn num_bits(&self) -> u32 {
        (N * 64) as u32 + (64 - self.1.leading_zeros())
    }

    /// Returns the `i`-th bit where bit 0 is the least significant one.
    /// In other words, the bit with weight `2^i`.
    pub(super) const fn get_bit(&self, i: usize) -> bool {
        let d = i / 64;
        let b = i % 64;
        if d == N {
            (self.1 >> b) & 1 == 1
        } else {
            (self.0[d] >> b) & 1 == 1
        }
    }
}

pub(super) struct R2Buffer<const N: usize>(pub [u64; N], pub [u64; N], pub u64);

impl<const N: usize> R2Buffer<N> {
    /// Find the number of bits in the binary decomposition of `self`.
    pub(super) const fn num_bits(&self) -> u32 {
        ((2 * N) * 64) as u32 + (64 - self.2.leading_zeros())
    }

    /// Returns the `i`-th bit where bit 0 is the least significant one.
    /// In other words, the bit with weight `2^i`.
    pub(super) const fn get_bit(&self, i: usize) -> bool {
        let d = i / 64;
        let b = i % 64;
        if d == 2 * N {
            (self.2 >> b) & 1 == 1
        } else if d >= N {
            (self.1[d - N] >> b) & 1 == 1
        } else {
            (self.0[d] >> b) & 1 == 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_const_for_macro() {
        let mut array = [0usize; 4];
        const_for!((i in 0..(array.len())) {
            array[i] = i;
        });
        assert_eq!(array, [0, 1, 2, 3]);
    }

    #[test]
    fn test_mul_buffer_new_and_get() {
        type Buf = MulBuffer<4>;
        let buf = Buf::new([1u64, 2u64, 3u64, 4u64], [5u64, 6u64, 7u64, 8u64]);

        assert_eq!(*buf.get(0), 1);
        assert_eq!(*buf.get(3), 4);
        assert_eq!(*buf.get(4), 5);
        assert_eq!(*buf.get(7), 8);
    }

    #[test]
    fn test_mul_buffer_get_mut() {
        type Buf = MulBuffer<4>;
        let mut buf = Buf::zeroed();
        *buf.get_mut(2) = 42;
        assert_eq!(buf.b0[2], 42);

        *buf.get_mut(5) = 99;
        assert_eq!(buf.b1[1], 99);
    }

    #[test]
    fn test_ser_buffer_zeroed_and_get() {
        type Ser = SerBuffer<2>;
        let buf = Ser::zeroed();
        assert_eq!(*buf.get(0), 0);
        assert_eq!(*buf.get(15), 0);
        assert_eq!(*buf.get(16), 0); // Check the `last` byte
    }

    #[test]
    fn test_ser_buffer_copy_from_u8_slice() {
        type Ser = SerBuffer<2>;
        let mut buf = Ser::zeroed();
        let data: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17];
        buf.copy_from_u8_slice(data);

        assert_eq!(buf.buffers[0], [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(buf.buffers[1], [9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(buf.last, 17);
    }

    #[test]
    fn test_ser_buffer_copy_from_u64_slice() {
        type Ser = SerBuffer<2>;
        let mut buf = Ser::zeroed();
        let data: &[u64; 2] = &[0x123456789ABCDEF0, 0x0FEDCBA987654321];
        buf.copy_from_u64_slice(data);

        assert_eq!(buf.buffers[0], 0x123456789ABCDEF0u64.to_le_bytes());
        assert_eq!(buf.buffers[1], 0x0FEDCBA987654321u64.to_le_bytes());
    }

    #[test]
    fn test_rbuffer_get_bit() {
        // Create an instance of RBuffer
        let buf = RBuffer([0x0, 0x8000000000000000], 0x1); // Second value has MSB set, and last has LSB set

        assert!(!buf.get_bit(63)); // Check the 63rd bit of the first part
        assert!(buf.get_bit(127)); // Check the MSB of the second part
        assert!(buf.get_bit(128)); // Check the LSB of the third part
    }

    #[test]
    fn test_ser_buffer_write_and_read() {
        type Ser = SerBuffer<2>;
        let buf = Ser::zeroed();
        let mut data = ark_std::vec::Vec::new();
        buf.write_up_to(&mut data, 16)
            .expect("Failed to write buffer");

        let mut new_buf = Ser::zeroed();
        new_buf
            .read_exact_up_to(&data[..], 16)
            .expect("Failed to read buffer");

        assert_eq!(buf.buffers, new_buf.buffers);
        assert_eq!(buf.last, new_buf.last);
    }

    #[test]
    fn test_mul_buffer_correctness() {
        type Buf = MulBuffer<10>;
        let temp = Buf::new([10u64; 10], [20u64; 10]);

        for i in 0..20 {
            if i < 10 {
                assert_eq!(temp[i], 10);
            } else {
                assert_eq!(temp[i], 20);
            }
        }
    }

    #[test]
    #[should_panic]
    fn test_mul_buffer_soundness() {
        type Buf = MulBuffer<10>;
        let temp = Buf::new([10u64; 10], [10u64; 10]);

        for i in 20..21 {
            // indexing `temp[20]` should panic
            assert_eq!(temp[i], 10);
        }
    }
}
