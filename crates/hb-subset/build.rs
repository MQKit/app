fn main() {
    let mut b = cc::Build::new();
    b.cpp(true).warnings(false).file("harfbuzz/src/harfbuzz-subset.cc").include("harfbuzz/src");
    if b.get_compiler().is_like_msvc() {
        b.flag("/bigobj").flag("/utf-8").flag("/std:c++17").flag("/EHs-c-").flag("/GR-");
    } else {
        b.flag("-std=c++17").flag("-fno-exceptions").flag("-fno-rtti");
    }
    b.compile("harfbuzz-subset");
    println!("cargo:rerun-if-changed=harfbuzz/src/harfbuzz-subset.cc");
}
