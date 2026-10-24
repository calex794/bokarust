// Same Rust HTML serialization code as our successful native experiment.
mod kernel { include!("../../../zig-kernel/render.rs"); }
const CAP: usize = 2 * 1024 * 1024;
static mut OUTPUT: [u8; CAP] = [0; CAP];
#[no_mangle]
pub extern "C" fn output_ptr() -> u32 {
    core::ptr::addr_of_mut!(OUTPUT) as *mut u8 as u32
}
#[no_mangle]
pub extern "C" fn output_cap() -> u32 { CAP as u32 }
#[no_mangle]
pub extern "C" fn render_html(ptr: *mut u8, cap: usize, n: usize) -> usize {
    kernel::render_html_rust(ptr, cap, n)
}
