use crate::{build, validate, AssetSource, Package};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use zip::write::SimpleFileOptions;

/// Create a minimal project tree in a unique temp dir.
fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vnengine_pack_{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("script")).unwrap();
    fs::create_dir_all(dir.join("images")).unwrap();
    fs::create_dir_all(dir.join("audio")).unwrap();
    fs::write(
        dir.join("project.toml"),
        r#"title = "Test Game"
width = 640
height = 480
fullscreen = false
vsync = true
entry_script = "script/main.vn"
image_dir = "images"
audio_dir = "audio"
voice_dir = "voice"
save_dir = "saves"
text_speed = 40.0
"#,
    )
    .unwrap();
    fs::write(
        dir.join("script/main.vn"),
        "label start:\n    \"hello\"\n    end\n",
    )
    .unwrap();
    fs::write(dir.join("images/bg.png"), b"not-a-real-png").unwrap();
    fs::write(dir.join("audio/bgm.ogg"), b"not-a-real-ogg").unwrap();
    fs::write(dir.join("LICENSE-MIT"), b"MIT license text").unwrap();
    dir
}

fn pkg_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("vnengine_pack_{name}.vnpkg"))
}

#[test]
fn build_and_open_roundtrip() {
    let project = fixture("roundtrip");
    let out = pkg_path("roundtrip");
    build(&project, &out).unwrap();

    let pkg = Package::open(&out).unwrap();
    assert_eq!(pkg.config().title, "Test Game");
    assert_eq!(pkg.manifest().format, crate::MANIFEST_FORMAT);
    assert_eq!(pkg.manifest().resolution, [640, 480]);
    // Compiled script loaded without parsing source.
    assert_eq!(pkg.script().labels.len(), 1);
    // Assets are readable by project-relative path.
    assert_eq!(
        pkg.read("images/bg.png").unwrap(),
        b"not-a-real-png".to_vec()
    );
    assert!(pkg.exists("audio/bgm.ogg"));
    assert!(!pkg.exists("audio/missing.ogg"));
    // Extension probing.
    let (name, _) = pkg.read_any("images/bg", &["png"]).unwrap();
    assert_eq!(name, "images/bg.png");
    // Asset index lists everything with hashes.
    let paths: Vec<&str> = pkg
        .manifest()
        .assets
        .iter()
        .map(|a| a.path.as_str())
        .collect();
    assert!(paths.contains(&"assets/images/bg.png"));
    assert!(paths.contains(&"assets/audio/bgm.ogg"));
    assert!(!pkg.manifest().licenses.is_empty());
    let _ = fs::remove_dir_all(&project);
    let _ = fs::remove_file(&out);
}

#[test]
fn build_is_deterministic() {
    let project = fixture("det");
    let a = pkg_path("det_a");
    let b = pkg_path("det_b");
    build(&project, &a).unwrap();
    build(&project, &b).unwrap();
    assert_eq!(
        fs::read(&a).unwrap(),
        fs::read(&b).unwrap(),
        "identical inputs must produce identical archives"
    );
    let _ = fs::remove_dir_all(&project);
    let _ = fs::remove_file(&a);
    let _ = fs::remove_file(&b);
}

#[test]
fn open_rejects_traversal_entry() {
    let path = pkg_path("traversal");
    {
        let file = fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("../evil.txt", options).unwrap();
        zip.write_all(b"evil").unwrap();
        zip.finish().unwrap();
    }
    let err = Package::open(&path).unwrap_err();
    assert!(err.to_string().contains("unsafe path"), "{err}");
    let _ = fs::remove_file(&path);
}

#[test]
fn open_rejects_missing_members() {
    let path = pkg_path("missing");
    {
        let file = fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(b"{}").unwrap();
        zip.finish().unwrap();
    }
    assert!(Package::open(&path).is_err());
    let _ = fs::remove_file(&path);
}

#[test]
fn open_rejects_bad_manifest_format() {
    let path = pkg_path("badfmt");
    {
        let file = fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(
            br#"{
            "format": 99,
            "engine": "vnengine",
            "engine_version": "0.1.0",
            "title": "x",
            "entrypoint": "script/main.vnb",
            "resolution": [1, 1],
            "assets": [],
            "licenses": []
        }"#,
        )
        .unwrap();
        zip.start_file("config.json", options).unwrap();
        zip.write_all(b"{}").unwrap();
        zip.start_file("script/main.vnb", options).unwrap();
        zip.write_all(b"{}").unwrap();
        zip.finish().unwrap();
    }
    let err = Package::open(&path).unwrap_err();
    assert!(err.to_string().contains("format"), "{err}");
    let _ = fs::remove_file(&path);
}

#[test]
fn asset_paths_reject_traversal() {
    let project = fixture("sanitize");
    let out = pkg_path("sanitize");
    build(&project, &out).unwrap();
    let pkg = Package::open(&out).unwrap();
    assert!(pkg.read("../project.toml").is_err());
    assert!(pkg.read("/abs/path").is_err());
    assert!(pkg.read("images/../project.toml").is_err());
    let _ = fs::remove_dir_all(&project);
    let _ = fs::remove_file(&out);
}

#[test]
fn validate_passes_on_fixture() {
    let project = fixture("validate");
    validate(&project).unwrap();
    let _ = fs::remove_dir_all(&project);
}

#[test]
fn build_fails_on_bad_script() {
    let project = fixture("badscript");
    fs::write(project.join("script/main.vn"), "label start:\n  \"unclosed").unwrap();
    let out = pkg_path("badscript");
    assert!(build(&project, &out).is_err());
    let _ = fs::remove_dir_all(&project);
}
