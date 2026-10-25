use front_rust_ui_compare::front_rust_probe_checksum;
fn main() {
    let count = std::env::args().nth(1)
        .expect("Usage: checksum <number-of-shapes>")
        .parse::<u32>()
        .expect("positive number");
    println!("{}", front_rust_probe_checksum(count));
}
