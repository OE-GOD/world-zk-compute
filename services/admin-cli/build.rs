fn main() {
    // Expose the target triple and build profile as compile-time env vars.
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=TARGET_TRIPLE={target}");
    println!("cargo:rustc-env=BUILD_PROFILE={profile}");
}
