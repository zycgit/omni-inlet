fn main() {
    for library in ["avformat", "avcodec", "swscale", "avutil"] {
        println!("cargo:rustc-link-lib={library}");
    }
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
}
