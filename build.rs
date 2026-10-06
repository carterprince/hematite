use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=data/hematite.gresource.xml");
    println!("cargo:rerun-if-changed=data/icons/scalable/apps/io.github.hematite.Editor.svg");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("hematite.gresource");
    let status = Command::new("glib-compile-resources")
        .arg("data/hematite.gresource.xml")
        .arg("--sourcedir=data")
        .arg("--target")
        .arg(output)
        .status()
        .expect("glib-compile-resources is required to embed the app icon");
    assert!(status.success(), "Could not compile the app icon resource");
    println!("cargo:rerun-if-changed=data/io.github.hematite.Editor.gschema.xml");
    let directory = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("schemas");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::copy(
        "data/io.github.hematite.Editor.gschema.xml",
        directory.join("io.github.hematite.Editor.gschema.xml"),
    )
    .unwrap();
    let status = Command::new("glib-compile-schemas")
        .arg("--strict")
        .arg(&directory)
        .status()
        .expect("glib-compile-schemas is required");
    assert!(status.success(), "Could not compile preferences schema");
}
