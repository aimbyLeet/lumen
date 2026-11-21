//! Builds libopus from libopus/ with `cc`: the float build without SIMD
//! (run-time CPU detection off) and without the DNN extensions of 1.5. The
//! source lists come from libopus's own `*_sources.mk`.

use std::path::Path;

/// The files of `var` in a libopus `.mk` list (`VAR = \` then one per line).
fn sources(mk: &Path, var: &str) -> Vec<String> {
    let text = std::fs::read_to_string(mk).unwrap_or_else(|e| panic!("{}: {e}", mk.display()));
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if !inside {
            inside = line.split('=').next().map(str::trim) == Some(var) && line.ends_with('\\');
            continue;
        }
        let file = line.trim_end_matches('\\').trim();
        if !file.is_empty() {
            out.push(file.to_string());
        }
        if !line.ends_with('\\') {
            break;
        }
    }
    assert!(!out.is_empty(), "no {var} in {}", mk.display());
    out
}

fn main() {
    let root = Path::new("libopus");
    let version = std::fs::read_to_string(root.join("package_version")).expect("package_version");
    let version = version
        .trim()
        .trim_start_matches("PACKAGE_VERSION=")
        .trim_matches('"')
        .to_string();
    let mut files = Vec::new();
    for (mk, var) in [
        ("opus_sources.mk", "OPUS_SOURCES"),
        ("opus_sources.mk", "OPUS_SOURCES_FLOAT"),
        ("celt_sources.mk", "CELT_SOURCES"),
        ("silk_sources.mk", "SILK_SOURCES"),
        ("silk_sources.mk", "SILK_SOURCES_FLOAT"),
    ] {
        files.extend(sources(&root.join(mk), var));
    }
    let mut b = cc::Build::new();
    b.include(root.join("include"))
        .include(root.join("celt"))
        .include(root.join("silk"))
        .include(root.join("silk/float"))
        .include(root.join("src"))
        .define("OPUS_BUILD", None)
        .define("USE_ALLOCA", None)
        .define("HAVE_LRINT", "1")
        .define("HAVE_LRINTF", "1")
        .define("PACKAGE_VERSION", format!("\"{version}\"").as_str())
        .warnings(false);
    for f in &files {
        b.file(root.join(f));
    }
    b.compile("opus");
    println!("cargo:rerun-if-changed=libopus");
}
