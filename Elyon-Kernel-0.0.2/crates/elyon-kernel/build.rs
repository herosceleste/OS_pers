fn main() {
    let linker =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("linker.ld");
    println!("cargo:rerun-if-changed={}", linker.display());
    println!("cargo:rustc-link-arg=-T{}", linker.display());
}
