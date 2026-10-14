//! Builds the dashboard (`dash/`) when `dash/dist` is missing or older than its sources,
//! and embeds `dash/dist` in the binary as `OUT_DIR/assets.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const INPUTS: [&str; 8] = [
    "dash/src",
    "dash/public",
    "dash/index.html",
    "dash/package.json",
    "dash/package-lock.json",
    "dash/vite.config.ts",
    "dash/tsconfig.json",
    "brand",
];

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dash = root.join("dash");
    let dist = dash.join("dist");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=dash/dist");
    for i in INPUTS {
        println!("cargo:rerun-if-changed={i}");
    }
    let newest = INPUTS
        .iter()
        .filter_map(|i| newest(&root.join(i)))
        .max();
    let built = std::fs::metadata(dist.join("index.html"))
        .and_then(|m| m.modified())
        .ok();
    if built.is_none() || newest.is_some_and(|n| Some(n) > built) {
        if !dash.join("node_modules").is_dir() {
            let install = if dash.join("package-lock.json").is_file() {
                "ci"
            } else {
                "install"
            };
            npm(&dash, &[install]);
        }
        npm(&dash, &["run", "build"]);
    }
    let mut files = Vec::new();
    walk(&dist, &mut files);
    files.sort();
    if !files.iter().any(|f| f == &dist.join("index.html")) {
        panic!("{} has no index.html after the build", dist.display());
    }
    let mut out = String::from("pub(crate) static ASSETS: &[(&str, &[u8])] = &[\n");
    for f in &files {
        let rel = f
            .strip_prefix(&dist)
            .expect("a file under dist")
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        out.push_str(&format!(
            "    ({rel:?}, include_bytes!({:?})),\n",
            f.display().to_string()
        ));
    }
    out.push_str("];\n");
    let dest = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("assets.rs");
    std::fs::write(&dest, out).expect("write assets.rs");
}

fn npm(dir: &Path, args: &[&str]) {
    let status = Command::new("npm")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("running npm {}: {e} (the dashboard needs Node.js)", args.join(" ")));
    if !status.success() {
        panic!("npm {} in {} exited with {status}", args.join(" "), dir.display());
    }
}

/// The newest modification time of a file, or of any file under a directory.
fn newest(p: &Path) -> Option<SystemTime> {
    let m = std::fs::metadata(p).ok()?;
    if !m.is_dir() {
        return m.modified().ok();
    }
    std::fs::read_dir(p)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| newest(&e.path()))
        .max()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}
