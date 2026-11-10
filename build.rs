//! Embeds the per-State Area files, `areas/<ST>.json`, into the binary, so
//! adding a State is adding a file (the format: `docs/areas.md`).

use std::fmt::Write;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=areas");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("areas");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("reading areas/")
        .map(|e| e.expect("reading areas/").path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let mut out = String::from(
        "/// `(file name, contents)` of every `areas/*.json`.\npub const AREA_FILES: &[(&str, &str)] = &[\n",
    );
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_name().unwrap().to_string_lossy();
        writeln!(
            out,
            "    ({name:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    let target = Path::new(&std::env::var("OUT_DIR").unwrap()).join("area_files.rs");
    std::fs::write(target, out).expect("writing area_files.rs");
}
