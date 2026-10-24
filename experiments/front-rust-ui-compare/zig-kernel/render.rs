// Rust equivalent of render.zig. No framework, no heap allocation per call.
struct Writer<'a> { dst: &'a mut [u8], pos: usize }
impl Writer<'_> {
    fn add(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > self.dst.len() - self.pos { return false; }
        let end = self.pos + bytes.len();
        self.dst[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
        true
    }
    fn number(&mut self, mut x: usize) -> bool {
        let mut tmp = [0u8;20];
        let mut len = 0;
        loop {
            tmp[len] = b'0' + (x % 10) as u8;
            len += 1;
            x /= 10;
            if x == 0 { break; }
        }
        while len > 0 {
            len -= 1;
            if !self.add(&tmp[len..len+1]) { return false; }
        }
        true
    }
}
#[no_mangle]
pub extern "C" fn render_html_rust(dst: *mut u8, cap: usize, count: usize) -> usize {
    if cap == 0 || dst.is_null() || !(3..=10000).contains(&count) { return 0; }
    // Caller guarantees a valid writable non-aliasing byte buffer.
    let bytes = unsafe { core::slice::from_raw_parts_mut(dst,cap) };
    let mut w=Writer { dst:bytes,pos:0 };
    macro_rules! add { ($x:expr) => {if !w.add($x) {return 0;}}; }
    macro_rules! num { ($x:expr) => {if !w.number($x) {return 0;}}; }
    add!(b"<main data-benchmark=\"ui-compare\"><h1>Rectangles</h1><section role=\"group\" aria-label=\"Rectangles\">");
    for id in 1..=count {
        let x=10+((id-1)%25)*130 + if id==2 {24} else {0};
        let y=20+((id-1)/25)*90 + if id==2 {8} else {0};
        add!(b"<div class=\"rect\" data-id=\"");
        num!(id);
        add!(b"\" data-x=\"");
        num!(x);
        add!(b"\" data-y=\"");
        num!(y);
        add!(b"\" role=\"img\" aria-label=\"Rectangle ");
        num!(id);
        add!(b"\" style=\"position:absolute;left:");
        num!(x);
        add!(b"px;top:");
        num!(y);
        add!(b"px\"></div>");
    }
    add!(b"</section></main>");
    w.pos
}
