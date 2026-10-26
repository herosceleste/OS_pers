fn main() {
    let linker =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("linker.ld");
    println!("cargo:rerun-if-changed={}", linker.display());
    println!("cargo:rerun-if-env-changed=ELYON_TEST_UD");
    println!("cargo:rerun-if-env-changed=ELYON_TEST_DF");
    println!("cargo:rerun-if-env-changed=ELYON_TEST_USER_IRET");
    println!("cargo:rerun-if-env-changed=ELYON_TEST_SMAP");
    println!("cargo:rerun-if-env-changed=ELYON_TEST_ALIAS");
    println!("cargo:rerun-if-env-changed=ELYON_BUILD_ID");
    println!("cargo:rustc-link-arg=-T{}", linker.display());
}
