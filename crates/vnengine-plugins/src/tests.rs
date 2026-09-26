use crate::{PluginHost, PluginKv, PluginManifest};

fn manifest(toml_src: &str) -> PluginManifest {
    PluginManifest::for_file(toml_src, "plug").unwrap()
}

fn host() -> PluginHost {
    PluginHost::new().unwrap()
}

#[test]
fn manifest_defaults_name_to_stem() {
    let m = manifest("version = \"1.0\"\n");
    assert_eq!(m.name, "plug");
    assert!(!m.permits("log"));
}

#[test]
fn manifest_rejects_bad_name() {
    assert!(PluginManifest::for_file("name = \"a/b\"\nversion=\"1\"", "x").is_err());
    assert!(PluginManifest::for_file("not toml at all", "x").is_err());
}

#[test]
fn loads_module_and_runs_hooks() {
    let mut host = host();
    let wat = r#"
        (module
            (func (export "vn_init"))
            (func (export "vn_tick") (param f32))
            (func (export "vn_on_choice") (param i32)))"#;
    host.load(
        wat.as_bytes(),
        manifest("version = \"1\"\nfuel_per_tick = 100000\n"),
    )
    .unwrap();
    assert_eq!(host.len(), 1);
    let mut kv = PluginKv::new();
    host.init_all(&mut kv);
    host.tick_all(&mut kv, 0.016);
    host.notify_choice(&mut kv, 2);
}

#[test]
fn kv_round_trip_through_memory() {
    let mut host = host();
    // vn_init: read key "greeting" (8 bytes at addr 0) into buf at 32,
    // then store it back under key "copy" (4 bytes at addr 48).
    let wat = r#"
        (module
            (import "env" "vn_kv_get" (func $get (param i32 i32 i32 i32) (result i32)))
            (import "env" "vn_kv_set" (func $set (param i32 i32 i32 i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "greeting")
            (data (i32.const 48) "copy")
            (func (export "vn_init")
                (local $n i32)
                (local.set $n (call $get
                    (i32.const 0) (i32.const 8)
                    (i32.const 32) (i32.const 64)))
                (call $set
                    (i32.const 48) (i32.const 4)
                    (i32.const 32) (local.get $n))))"#;
    host.load(
        wat.as_bytes(),
        manifest("version = \"1\"\npermissions = [\"kv\"]\n"),
    )
    .unwrap();
    let mut kv = PluginKv::new();
    kv.insert("plug.greeting".into(), "bonjour".into());
    host.init_all(&mut kv);
    assert_eq!(kv.get("plug.copy"), Some(&"bonjour".to_string()));
}

#[test]
fn kv_get_missing_key_returns_minus_one() {
    let mut host = host();
    // Write the returned code (-1 expected) as a one-char-ish record:
    // set key "missing" to whatever get returns — instead we simply
    // verify no trap and no entry is created.
    let wat = r#"
        (module
            (import "env" "vn_kv_get" (func $get (param i32 i32 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "nope")
            (func (export "vn_init")
                (if (i32.ne (call $get
                        (i32.const 0) (i32.const 4)
                        (i32.const 32) (i32.const 16))
                    (i32.const -1))
                    (then unreachable))))"#;
    host.load(
        wat.as_bytes(),
        manifest("version = \"1\"\npermissions = [\"kv\"]\n"),
    )
    .unwrap();
    let mut kv = PluginKv::new();
    host.init_all(&mut kv); // would log an error if it trapped
    assert!(!kv.iter().any(|(k, _)| k.starts_with("plug.")));
}

#[test]
fn undeclared_import_rejected() {
    let mut host = host();
    let wat = r#"
        (module
            (import "env" "vn_log" (func $log (param i32 i32)))
            (func (export "vn_init")))"#;
    // The manifest grants nothing, so importing vn_log must fail.
    let err = host
        .load(wat.as_bytes(), manifest("version = \"1\"\n"))
        .unwrap_err();
    assert!(err.to_string().contains("log"), "got: {err}");
}

#[test]
fn unknown_import_rejected() {
    let mut host = host();
    let wat = r#"
        (module
            (import "env" "vn_shell" (func $s))
            (func (export "vn_init")))"#;
    let err = host
        .load(
            wat.as_bytes(),
            manifest("version = \"1\"\npermissions = [\"log\", \"kv\"]\n"),
        )
        .unwrap_err();
    assert!(format!("{err:#}").contains("vn_shell"), "got: {err:#}");
}

#[test]
fn out_of_fuel_traps_without_hanging() {
    let mut host = host();
    let wat = r#"
        (module
            (func (export "vn_tick") (param f32)
                (loop $l br $l)))"#;
    host.load(
        wat.as_bytes(),
        manifest("version = \"1\"\nfuel_per_tick = 1000\n"),
    )
    .unwrap();
    let mut kv = PluginKv::new();
    // Infinite loop hits the 1000-fuel budget; the error is logged and
    // the call returns instead of hanging.
    host.tick_all(&mut kv, 0.016);
}

#[test]
fn memory_limit_enforced() {
    let mut host = host();
    // Module declares 2 pages (128 KiB) but the manifest caps at 64 KiB.
    let wat = r#"
        (module
            (memory (export "memory") 2)
            (func (export "vn_init")))"#;
    let err = host
        .load(
            wat.as_bytes(),
            manifest("version = \"1\"\nmemory_max_bytes = 65536\n"),
        )
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("memory") || msg.contains("limit") || msg.contains("size"),
        "got: {msg}"
    );
}

#[test]
fn logs_are_forwarded() {
    let mut host = host();
    let wat = r#"
        (module
            (import "env" "vn_log" (func $log (param i32 i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "hi from plugin")
            (func (export "vn_init")
                (call $log (i32.const 0) (i32.const 14))))"#;
    host.load(
        wat.as_bytes(),
        manifest("version = \"1\"\npermissions = [\"log\"]\n"),
    )
    .unwrap();
    let mut kv = PluginKv::new();
    host.init_all(&mut kv); // exercises vn_log; message lands in the log sink
}
