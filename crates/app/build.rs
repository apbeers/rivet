fn main() {
    println!("cargo:rerun-if-changed=../../tools/windows.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("../../tools/windows.manifest");
        println!("cargo:rustc-link-arg-bin=rivet=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=rivet=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
