fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Cargo owns the profile. Nested packaged-plugin builds must use the same artifact class
    // as the harness, including release builds with debug assertions explicitly enabled.
    let profile = std::env::var("PROFILE")?;
    println!("cargo:rustc-env=REVY_PACKAGED_PLUGIN_PROFILE={profile}");
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}
