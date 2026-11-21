use std::{env, fs, path::PathBuf};

// Windows reserves 1 MiB for a program's main thread, which the unoptimised
// CLI and MCP entries exceed on browser paths. Reserve the 8 MiB Linux gives.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:8388608");
    }
    // Scripts that read target ownership mark the shared prelude's place; `browser_script!`
    // includes them with the prelude spliced in, so there is one copy of it to review.
    println!("cargo:rerun-if-changed=browser-scripts");
    let prelude = fs::read_to_string("browser-scripts/ownership-prelude.js").unwrap();
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("browser-scripts");
    fs::create_dir_all(&out).unwrap();
    for entry in fs::read_dir("browser-scripts").unwrap() {
        let path = entry.unwrap().path();
        let script = fs::read_to_string(&path).unwrap();
        if script.contains("/* ownership-prelude.js */") {
            let script = script.replace("/* ownership-prelude.js */", prelude.trim());
            fs::write(out.join(path.file_name().unwrap()), script).unwrap();
        }
    }
}
