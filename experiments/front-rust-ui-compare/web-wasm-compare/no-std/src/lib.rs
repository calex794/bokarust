#![no_std]
// The identical Rust serialization source is compiled without libstd.
// It has no allocator, unwind system or diagnostic formatting runtime.
// Fatal panics spin instead of providing std diagnostics: this is a size
// experiment, not a recommended error-handling approach for applications.
use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    loop {}
}

mod kernel {
    include!("../../../zig-kernel/render.rs");
}

const CAP: usize = 2 * 1024 * 1024;
static mut OUTPUT: [u8; CAP] = [0; CAP];

#[no_mangle]
pub extern "C" fn output_ptr() -> u32 {
    core::ptr::addr_of_mut!(OUTPUT) as *mut u8 as u32
}

#[no_mangle]
pub extern "C" fn output_cap() -> u32 {
    CAP as u32
}

#[no_mangle]
pub extern "C" fn render_html(ptr: *mut u8, cap: usize, n: usize) -> usize {
    kernel::render_html_rust(ptr, cap, n)
}
