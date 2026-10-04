#![cfg(all(feature = "std", feature = "encoder"))]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    io::{self, Write},
};

use lzma_rust2::filter::bcj2::{Bcj2Options, Bcj2Writer};

struct Allocator;

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

thread_local! {
    static USAGE: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

fn update(added: usize, removed: usize) {
    let _ = USAGE.try_with(|usage| {
        if let Some((current, peak)) = usage.get() {
            let current = current + added - removed;
            usage.set(Some((current, peak.max(current))));
        }
    });
}

// The allocator forwards each operation and its original layout to System.
// Accounting is confined to objects allocated and dropped by the measured closure.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller provides a valid layout, forwarded unchanged.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            update(layout.size(), 0);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: GlobalAlloc's caller provides a valid layout, forwarded unchanged.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            update(layout.size(), 0);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        update(0, layout.size());
        // SAFETY: The pointer and original allocation layout are forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: The live pointer, original layout and valid new size come from the caller.
        let result = unsafe { System.realloc(ptr, layout, size) };
        if !result.is_null() {
            update(size, layout.size());
        }
        result
    }
}

fn peak(f: impl FnOnce()) -> usize {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            USAGE.with(|usage| usage.set(None));
        }
    }
    USAGE.with(|usage| usage.set(Some((0, 0))));
    let _reset = Reset;
    f();
    USAGE.with(|usage| usage.get().unwrap().1)
}

#[derive(Default)]
struct CountingSink {
    size: usize,
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.size += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn buffering_stays_bounded_for_large_writes_and_streams() {
    let pattern = [
        0xE8, 0, 0, 0, 0, 0xE9, 0xFB, 0xFF, 0xFF, 0xFF, 0x0F, 0x85, 0xF5, 0xFF, 0xFF, 0xFF,
    ];
    let data = pattern.repeat((1 << 24) / pattern.len());
    for size in [0, 1 << 16, 1 << 20, 1 << 24] {
        for chunk_size in [1 << 15, 1 << 24] {
            let allocated = peak(|| {
                let mut writer = Bcj2Writer::new(
                    std::array::from_fn(|_| CountingSink::default()),
                    &Bcj2Options::default(),
                )
                .unwrap();
                for chunk in data[..size].chunks(chunk_size) {
                    writer.write_all(chunk).unwrap();
                    writer.flush().unwrap();
                }
                let [main, call, jump, rc] = writer.finish().unwrap();
                assert_eq!(main.size + call.size + jump.size, size);
                assert!(rc.size >= 5);
            });
            println!("input={size} chunk={chunk_size} peak_payload={allocated}");
            assert!(
                allocated <= (1 << 16) + 4096,
                "BCJ2 buffering requested {allocated} bytes"
            );
        }
    }
}
