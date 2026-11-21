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
fn build_includes_plugins() {
    let project = fixture("plugins");
    fs::create_dir_all(project.join("plugins")).unwrap();
    fs::write(project.join("plugins/codex.wasm"), b"\0asm fake").unwrap();
    fs::write(
        project.join("plugins/codex.toml"),
        "version = \"1\"\npermissions = [\"log\", \"kv\"]\n",
    )
    .unwrap();
    // Non-wasm/toml files are skipped.
    fs::write(project.join("plugins/readme.md"), b"docs").unwrap();
    let out = pkg_path("plugins");
    build(&project, &out).unwrap();

    let pkg = Package::open(&out).unwrap();
    assert!(pkg
        .manifest()
        .plugins
        .contains(&"assets/plugins/codex.wasm".into()));
    assert!(pkg.exists("plugins/codex.wasm"));
    assert!(pkg.exists("plugins/codex.toml"));
    assert!(!pkg.exists("plugins/readme.md"));
    // Listing works through the AssetSource view.
    let listed = pkg.list("plugins");
    assert!(listed.contains(&"plugins/codex.wasm".to_string()));
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

#[test]
fn custom_font_directory_is_packaged() {
    let project = fixture("font_dir");
    let out = pkg_path("font_dir");
    let config = fs::read_to_string(project.join("project.toml")).unwrap();
    fs::write(
        project.join("project.toml"),
        format!("{config}\nfont_dir = \"typefaces\"\n"),
    )
    .unwrap();
    fs::create_dir_all(project.join("typefaces/nested")).unwrap();
    fs::write(project.join("typefaces/nested/test.ttf"), b"font data").unwrap();
    build(&project, &out).unwrap();
    let pkg = Package::open(&out).unwrap();
    assert_eq!(pkg.config().font_dir, "typefaces");
    assert_eq!(pkg.list("typefaces"), vec!["typefaces/nested/test.ttf"]);
    assert_eq!(pkg.read("typefaces/nested/test.ttf").unwrap(), b"font data");
    let _ = fs::remove_dir_all(project);
    let _ = fs::remove_file(out);
}

#[test]
fn build_with_imports_merges_ast_and_ships_sources() {
    let project = fixture("imports");
    fs::create_dir_all(project.join("script/chapters")).unwrap();
    fs::write(
        project.join("script/main.vn"),
        "import \"chapters/ch1.vn\"\ndefine e = Character(\"E\", color = \"#ffffff\")\nlabel start:\n    e \"hi\"\n    jump ch1\n    end\n",
    )
    .unwrap();
    fs::write(
        project.join("script/chapters/ch1.vn"),
        "label ch1:\n    \"chapter 1\"\n    end\n",
    )
    .unwrap();
    // A .vn file no import reaches must not be shipped.
    fs::write(project.join("script/unused.vn"), "label unused:\n    end\n").unwrap();
    let out = pkg_path("imports");
    build(&project, &out).unwrap();
    let pkg = Package::open(&out).unwrap();
    let names: Vec<&str> = pkg
        .script()
        .labels
        .iter()
        .map(|l| l.name.as_str())
        .collect();
    assert!(names.contains(&"start"), "labels: {:?}", names);
    assert!(names.contains(&"ch1"), "labels: {:?}", names);
    assert_eq!(pkg.script().definitions.len(), 1);
    // Raw .vn sources are shipped under script/ as plain archive members
    // (not under assets/).
    let file = fs::File::open(&out).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.contains(&"script/main.vn".to_string()), "{names:?}");
    assert!(
        names.contains(&"script/chapters/ch1.vn".to_string()),
        "{names:?}"
    );
    assert!(
        !names.contains(&"script/unused.vn".to_string()),
        "{names:?}"
    );
}

#[test]
fn check_reports_import_cycle_with_chain() {
    let project = fixture("cycle");
    fs::write(
        project.join("script/main.vn"),
        "import \"b.vn\"\nlabel start:\n    end\n",
    )
    .unwrap();
    fs::write(
        project.join("script/b.vn"),
        "import \"main.vn\"\nlabel b:\n    end\n",
    )
    .unwrap();
    let report = crate::check(&project).unwrap();
    assert!(!report.ok);
    let has_cycle = report
        .diagnostics
        .iter()
        .any(|d| d.diagnostic.message.contains("import cycle detected"));
    assert!(has_cycle, "diagnostics: {:?}", report.diagnostics);
}
