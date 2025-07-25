use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use core::{
    marker::PhantomData,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

/// Aligned memory to store i32. Aligned to 64 bytes, which is 256 bit.
/// This is perfect for 128-bit, 256-bit SIMD and is a cache line on modern CPU.
pub(crate) struct AlignedMemoryI32 {
    ptr: NonNull<u8>,
    layout: Layout,
    target_length: usize,
    _marker: PhantomData<i32>,
}

unsafe impl Send for AlignedMemoryI32 {}

unsafe impl Sync for AlignedMemoryI32 {}

impl AlignedMemoryI32 {
    /// Allocates memory that is at least "min_length"'s i32 in size.
    pub(crate) fn new(min_length: usize) -> Self {
        const ALIGNMENT: usize = 64;

        assert_ne!(size_of::<i32>(), 0);
        assert!(size_of::<i32>() <= ALIGNMENT);
        assert_eq!(ALIGNMENT % size_of::<i32>(), 0);

        assert_ne!(align_of::<i32>(), 0);
        assert!(align_of::<i32>() <= ALIGNMENT);
        assert_eq!(ALIGNMENT % align_of::<i32>(), 0);

        let required_bytes = (min_length * size_of::<i32>()).div_ceil(ALIGNMENT) * ALIGNMENT;
        let target_length = required_bytes / size_of::<i32>();

        let layout = Layout::from_size_align(required_bytes, ALIGNMENT).expect("invalid layout");
        // SAFETY: We created a proper layout.
        let ptr = unsafe { alloc_zeroed(layout) };
        let ptr = NonNull::new(ptr).expect("failed to allocate memory");

        Self {
            ptr,
            layout,
            target_length,
            _marker: PhantomData,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.target_length
    }
}

impl Deref for AlignedMemoryI32 {
    type Target = [i32];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl DerefMut for AlignedMemoryI32 {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

impl AsRef<[i32]> for AlignedMemoryI32 {
    fn as_ref(&self) -> &[i32] {
        // SAFETY: Points to a valid region in space that is allocated, aligned, initialized and
        // of length target_length * size_of::<i32>()
        unsafe { core::slice::from_raw_parts(self.ptr.cast::<i32>().as_ptr(), self.target_length) }
    }
}

impl AsMut<[i32]> for AlignedMemoryI32 {
    fn as_mut(&mut self) -> &mut [i32] {
        // SAFETY: Points to a valid region in space that is allocated, aligned, initialized and
        // of length target_length * size_of::<i32>()
        unsafe {
            core::slice::from_raw_parts_mut(self.ptr.cast::<i32>().as_ptr(), self.target_length)
        }
    }
}

impl Drop for AlignedMemoryI32 {
    fn drop(&mut self) {
        if self.layout.size() > 0 {
            // SAFETY: We use the original ptr and layout we allocated this memory for.
            unsafe { dealloc(self.ptr.as_ptr(), self.layout) };
        }
    }
}
