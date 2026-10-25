// Zig 0.17.0: framework-free HTML formatting, same C ABI and bytes as Rust.
const Writer = struct {
    dst: [*]u8,
    cap: usize,
    pos: usize = 0,
    fn add(self: *Writer, text: []const u8) bool {
        if (text.len > self.cap - self.pos) return false;
        @memcpy(self.dst[self.pos .. self.pos + text.len], text);
        self.pos += text.len;
        return true;
    }
    fn number(self: *Writer, value: usize) bool {
        var x = value;
        var tmp: [20]u8 = undefined;
        var len: usize = 0;
        while (true) {
            tmp[len] = @as(u8, @intCast(x % 10)) + '0';
            len += 1;
            x = @divTrunc(x, 10);
            if (x == 0) break;
        }
        while (len > 0) {
            len -= 1;
            if (!self.add(tmp[len .. len + 1])) return false;
        }
        return true;
    }
};
pub export fn render_html_zig(dst: [*]u8, capacity: usize, count: usize) usize {
    if (capacity == 0 or count < 3 or count > 10000) return 0;
    var w = Writer{ .dst = dst, .cap = capacity };
    if (!w.add("<main data-benchmark=\"ui-compare\"><h1>Rectangles</h1><section role=\"group\" aria-label=\"Rectangles\">")) return 0;
    var id: usize = 1;
    while (id <= count) : (id += 1) {
        const x: usize = 10 + ((id - 1) % 25) * 130 + (if (id == 2) @as(usize, 24) else 0);
        const y: usize = 20 + @divTrunc(id - 1, 25) * 90 + (if (id == 2) @as(usize, 8) else 0);
        if (!w.add("<div class=\"rect\" data-id=\"")) return 0;
        if (!w.number(id)) return 0;
        if (!w.add("\" data-x=\"")) return 0;
        if (!w.number(x)) return 0;
        if (!w.add("\" data-y=\"")) return 0;
        if (!w.number(y)) return 0;
        if (!w.add("\" role=\"img\" aria-label=\"Rectangle ")) return 0;
        if (!w.number(id)) return 0;
        if (!w.add("\" style=\"position:absolute;left:")) return 0;
        if (!w.number(x)) return 0;
        if (!w.add("px;top:")) return 0;
        if (!w.number(y)) return 0;
        if (!w.add("px\"></div>")) return 0;
    }
    if (!w.add("</section></main>")) return 0;
    return w.pos;
}
