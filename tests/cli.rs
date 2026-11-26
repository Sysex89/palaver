#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;

const DOCUMENT: &str =
    r#"{"nodes":[{"id":1,"x":0,"y":0,"speaker":"Guide","text":"Welcome"}],"edges":[]}"#;

#[test]
fn exports_accept_non_utf8_source_and_destination_paths() {
    let folder = tempfile::tempdir().unwrap();
    let source = folder.path().join(OsStr::from_bytes(b"talk-\xff.dln"));
    let destination = folder.path().join(OsStr::from_bytes(b"strings-\xfe.ftl"));
    fs::write(&source, DOCUMENT).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_palaver"))
        .arg(&source)
        .arg("--export-ftl")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(
        fs::read_to_string(destination)
            .unwrap()
            .contains("node-1 = Welcome")
    );
}

#[test]
fn export_creates_the_target_of_a_dangling_symlink_chain() {
    let folder = tempfile::tempdir().unwrap();
    let source = folder.path().join("talk.dln");
    let destination = folder.path().join("strings.ftl");
    let links = folder.path().join("links");
    let intermediate = links.join("alias.ftl");
    let target = folder.path().join("new.ftl");
    fs::write(&source, DOCUMENT).unwrap();
    fs::create_dir(&links).unwrap();
    symlink(&intermediate, &destination).unwrap();
    symlink("../new.ftl", &intermediate).unwrap();
    assert!(!destination.exists());

    let output = Command::new(env!("CARGO_BIN_EXE_palaver"))
        .arg(&source)
        .arg("--export-ftl")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(fs::read_link(destination).unwrap(), intermediate);
    assert_eq!(
        fs::read_link(intermediate).unwrap(),
        Path::new("../new.ftl")
    );
    assert!(
        fs::read_to_string(target)
            .unwrap()
            .contains("node-1 = Welcome")
    );
}

#[test]
fn a_failed_export_leaves_the_symlink_in_place() {
    let folder = tempfile::tempdir().unwrap();
    let source = folder.path().join("talk.dln");
    let destination = folder.path().join("strings.ftl");
    let target = Path::new("missing/strings.ftl");
    fs::write(&source, DOCUMENT).unwrap();
    symlink(target, &destination).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_palaver"))
        .arg(&source)
        .arg("--export-ftl")
        .arg(&destination)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_link(destination).unwrap(), target);
}
