// Embeds the icon and the manifest with the Windows SDK's rc.exe. A machine without the SDK still
// builds: the program then has the default icon, and asks for DPI awareness at run time anyway.
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_rc() -> Option<PathBuf> {
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(kits).ok()?.flatten().map(|e| e.path()).collect();
    versions.sort();
    versions.reverse();
    versions.into_iter().map(|v| v.join("x64").join("rc.exe")).find(|p| p.exists())
}

fn main() {
    println!("cargo:rerun-if-changed=assets/lists/cities.json");
    println!("cargo:rerun-if-changed=assets/lists/polygons.json");
    // (Tzofar's place lists are not in the repository: they are fetched once before the first build)
    for list in ["assets/lists/cities.json", "assets/lists/polygons.json"] {
        if !Path::new(list).exists() {
            panic!("{list} is missing: run `python tools/fetch_lists.py` once, then build again");
        }
    }
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if !Path::new("assets/app.rc").exists() || !Path::new("assets/icon.ico").exists() {
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found: no icon in the exe");
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.res");
    let status = Command::new(rc).args(["/nologo", "/fo"]).arg(&out).arg("assets/app.rc").status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", out.display()),
        other => println!("cargo:warning=rc.exe failed: {other:?}"),
    }
}
