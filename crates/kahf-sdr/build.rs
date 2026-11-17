// Links libhackrf and librtlsdr from Homebrew (macOS) or the system path (Linux).
fn main() {
    for p in ["/opt/homebrew/lib", "/usr/local/lib", "/usr/lib"] {
        println!("cargo:rustc-link-search=native={p}");
    }
    println!("cargo:rustc-link-lib=hackrf");
    println!("cargo:rustc-link-lib=rtlsdr");
}
