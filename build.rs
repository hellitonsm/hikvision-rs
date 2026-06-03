fn main() {
    // Add hikvision-libs to the dynamic linker RUNPATH so DT_NEEDED
    // dependencies (libAudioRender.so, libSuperRender.so loaded via
    // libPlayCtrl.so at process startup) can be resolved. RUNPATH also
    // works for dlopen() calls from within the SDK.
    //
    // $ORIGIN resolves to the directory containing the binary:
    //   target/debug/hikvision-rs  →  $ORIGIN/../../hikvision-libs
    //   target/release/hikvision-rs → $ORIGIN/../../hikvision-libs
    //
    // Use rustc-link-arg (not rustc-link-arg-bin) because the default
    // binary name may not match the explicit target name.
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,$ORIGIN/../../hikvision-libs");

    // Also link PlayCtrl and its deps at compile time so the dynamic
    // linker loads them before any dlopen() call, matching rustdemo's
    // approach in its build.rs.
    println!("cargo:rustc-link-search=native={}/hikvision-libs",
        std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rustc-link-lib=dylib=PlayCtrl");
    println!("cargo:rustc-link-lib=dylib=AudioRender");
    println!("cargo:rustc-link-lib=dylib=SuperRender");
}
