fn main() {
    println!("cargo:rerun-if-env-changed=JVL_VERSION");
    if let Ok(version) = std::env::var("JVL_VERSION") {
        println!("cargo:rustc-env=CARGO_PKG_VERSION={version}");
    }
}
