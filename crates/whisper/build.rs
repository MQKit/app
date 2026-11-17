//! whisper.cpp with CMake (CPU only, static), then the C shim with cc.

fn main() {
    // The C runtime the rest of the program uses: static in a release (package.ps1 builds
    // with +crt-static, so the app starts on a PC without the Visual C++ runtime), the DLL
    // otherwise. Mixing the two fails to link.
    let crt_static = std::env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default().split(',').any(|f| f == "crt-static");
    let (crt, crt_name) = if crt_static { ("/MT", "MultiThreaded") } else { ("/MD", "MultiThreadedDLL") };
    // whisper.cpp names its source files in its error messages (__FILE__): without this,
    // the program would carry the folder of the PC that built it. Trimmed to whisper.cpp\...
    let here = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let release_flags = format!("{crt} /O2 /Ob2 /DNDEBUG /d1trimfile:{here}");
    let dst = cmake::Config::new("cmake")
        .profile("Release")
        .static_crt(crt_static)
        // The cmake crate replaces CMake's release flags with its own (no /O2, no NDEBUG):
        // whisper would run several times slower. Given here, they are kept.
        .define("CMAKE_C_FLAGS_RELEASE", &release_flags)
        .define("CMAKE_CXX_FLAGS_RELEASE", &release_flags)
        .define("CMAKE_MSVC_RUNTIME_LIBRARY", crt_name)
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("WHISPER_BUILD_TESTS", "OFF")
        .define("WHISPER_BUILD_EXAMPLES", "OFF")
        .define("WHISPER_BUILD_SERVER", "OFF")
        .define("WHISPER_CURL", "OFF")
        .define("WHISPER_SDL2", "OFF")
        .define("GGML_BUILD_TESTS", "OFF")
        .define("GGML_BUILD_EXAMPLES", "OFF")
        .define("GGML_OPENMP", "OFF")
        .define("GGML_CCACHE", "OFF")
        // Not tuned to the build PC: any x86-64 processor with AVX2 (2013 on) runs it.
        .define("GGML_NATIVE", "OFF")
        .define("GGML_SSE42", "ON")
        .define("GGML_AVX", "ON")
        .define("GGML_AVX2", "ON")
        .define("GGML_FMA", "ON")
        .define("GGML_F16C", "ON")
        .define("GGML_BMI2", "ON")
        .define("GGML_AVX512", "OFF")
        .build();
    for sub in ["lib", "lib64"] {
        println!("cargo:rustc-link-search=native={}", dst.join(sub).display());
    }
    cc::Build::new()
        .cpp(true)
        .file("shim.cpp")
        .include("whisper.cpp/include")
        .include("whisper.cpp/ggml/include")
        .flag_if_supported("/utf-8")
        .compile("whisper_shim");
    for lib in ["whisper", "ggml", "ggml-cpu", "ggml-base"] {
        println!("cargo:rustc-link-lib=static={lib}");
    }
    // ggml reads the processor name from the registry.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=advapi32");
    }
    println!("cargo:rerun-if-changed=shim.cpp");
    println!("cargo:rerun-if-changed=whisper.cpp/src");
}
