fn main() {
    link_compiler_rt_on_macos();
    tauri_build::build()
}

/// llama.cpp's Metal backend uses Objective-C `@available` checks, which
/// compile to `___isPlatformVersionAtLeast` from clang's compiler-rt. rustc
/// links with `-nodefaultlibs`, so in release (LTO) builds that symbol is
/// missing unless we link the runtime explicitly.
fn link_compiler_rt_on_macos() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let Ok(out) = std::process::Command::new("clang").arg("--print-resource-dir").output() else {
        println!("cargo:warning=clang not found; skipping compiler-rt link");
        return;
    };
    let resource_dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let lib_dir = format!("{resource_dir}/lib/darwin");
    if !std::path::Path::new(&format!("{lib_dir}/libclang_rt.osx.a")).exists() {
        println!("cargo:warning=libclang_rt.osx.a not found in {lib_dir}");
        return;
    }
    println!("cargo:rustc-link-search=native={lib_dir}");
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}
