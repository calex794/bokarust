fn main() {
    // Rebuild embedded migrations when a SQL file is added or changed.
    println!("cargo:rerun-if-changed=migrations");
}
