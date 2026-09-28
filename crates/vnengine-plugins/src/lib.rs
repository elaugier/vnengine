//! Sandboxed WebAssembly plugins.
//!
//! A plugin is a `.wasm` module plus a `<name>.toml` manifest declaring
//! the permissions it uses. The host runs plugins through wasmtime with
//! fuel limits (CPU budget per call) and memory limits, and only wires
//! the host functions the manifest declares.
//!
//! ## Plugin ABI
//!
//! Host imports (`env` module), gated by permissions:
//!
//! - `vn_log(ptr, len)` — log a UTF-8 message (permission `log`)
//! - `vn_kv_set(kptr, klen, vptr, vlen)` — write a key (permission `kv`)
//! - `vn_kv_get(kptr, klen, dptr, dcap) -> i32` — read a key into a
//!   plugin-provided buffer; returns bytes written, or -1 (permission `kv`)
//!
//! Plugin exports (all optional unless noted):
//!
//! - `memory` — linear memory (required for the host functions above)
//! - `vn_init()` — called once after load
//! - `vn_tick(dt: f32)` — called each engine tick
//! - `vn_on_choice(index: i32)` — called when the player picks a menu item
//! - `vn_alloc(len: i32) -> ptr` — allocate `len` bytes; required for
//!   `vn_on_command` (the host writes the JSON payload at `ptr`)
//! - `vn_on_command(ptr, len)` — called for each `plugin <name> ...`
//!   statement in a script; `ptr..ptr+len` is a JSON array of args
//!
//! Plugin state lives in the runtime's key/value store (namespaced as
//! `<plugin>.<key>`), which is part of saves and rollback snapshots —
//! so plugin state restores automatically with the game.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wasmtime::{
    Caller, Config, Engine, Instance, Linker, Module, Store, StoreLimits, StoreLimitsBuilder,
};

/// Shared plugin state, owned by the runtime and passed to calls.
pub type PluginKv = BTreeMap<String, String>;

fn default_fuel() -> u64 {
    1_000_000
}
fn default_memory() -> usize {
    32 * 1024 * 1024
}

/// Declared in `<name>.toml` next to the wasm module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name; also the KV namespace prefix. Defaults to the wasm
    /// file stem when deserialized through [`PluginManifest::for_file`].
    #[serde(default)]
    pub name: String,
    pub version: String,
    /// Permissions the plugin needs: `log`, `kv`.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Fuel budget per host call (CPU limit).
    #[serde(default = "default_fuel")]
    pub fuel_per_tick: u64,
    /// Linear memory cap in bytes.
    #[serde(default = "default_memory")]
    pub memory_max_bytes: usize,
}

impl PluginManifest {
    /// Parse a manifest, defaulting `name` to the wasm file stem.
    pub fn for_file(toml_src: &str, file_stem: &str) -> Result<Self> {
        let mut m: PluginManifest = toml::from_str(toml_src).context("invalid plugin manifest")?;
        if m.name.is_empty() {
            m.name = file_stem.to_string();
        }
        if m.name.chars().any(|c| !c.is_alphanumeric() && c != '_') {
            bail!("invalid plugin name `{}`", m.name);
        }
        Ok(m)
    }

    pub fn permits(&self, perm: &str) -> bool {
        self.permissions.iter().any(|p| p == perm)
    }
}

/// The permission each host import requires.
fn import_permission(name: &str) -> Result<&'static str> {
    match name {
        "vn_log" => Ok("log"),
        "vn_kv_get" | "vn_kv_set" => Ok("kv"),
        other => bail!("unknown host import `env.{other}`"),
    }
}

/// Per-plugin host state, swapped with the runtime KV around each call.
struct HostState {
    plugin: String,
    kv: PluginKv,
    logs: Vec<String>,
    limits: StoreLimits,
}

/// A loaded, instantiated plugin.
pub struct Plugin {
    manifest: PluginManifest,
    store: Store<HostState>,
    instance: Instance,
}

impl Plugin {
    pub fn name(&self) -> &str {
        &self.manifest.name
    }
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Call an exported function if present, with the fuel budget applied.
    fn call<I, O>(&mut self, name: &str, params: I) -> Result<Option<O>>
    where
        I: wasmtime::WasmParams,
        O: wasmtime::WasmResults,
    {
        let Ok(func) = self.instance.get_typed_func::<I, O>(&mut self.store, name) else {
            return Ok(None);
        };
        self.store
            .set_fuel(self.manifest.fuel_per_tick)
            .map_err(|e| anyhow::anyhow!("fuel: {e}"))?;
        match func.call(&mut self.store, params) {
            Ok(out) => Ok(Some(out)),
            Err(e) => {
                if e.downcast_ref::<wasmtime::Trap>()
                    .map(|t| matches!(t, wasmtime::Trap::OutOfFuel))
                    .unwrap_or(false)
                {
                    bail!("plugin `{}` ran out of fuel", self.manifest.name);
                }
                Err(e.into())
            }
        }
    }

    /// Copy `payload` into plugin memory via `vn_alloc` and invoke
    /// `vn_on_command`. Returns false when the plugin doesn't export
    /// the command hook.
    fn deliver(&mut self, payload: &[u8]) -> Result<bool> {
        if self
            .instance
            .get_func(&mut self.store, "vn_on_command")
            .is_none()
        {
            return Ok(false);
        }
        let len = i32::try_from(payload.len()).context("payload too large")?;
        let Some(ptr) = self.call::<(i32,), i32>("vn_alloc", (len,))? else {
            bail!("plugin `{}` exports no `vn_alloc`", self.manifest.name);
        };
        let mem = self
            .instance
            .get_memory(&mut self.store, "memory")
            .context("plugin exports no `memory`")?;
        mem.write(&mut self.store, ptr as usize, payload)
            .context("payload write out of bounds")?;
        self.call::<(i32, i32), ()>("vn_on_command", (ptr, len))?;
        Ok(true)
    }
}

/// The plugin host: engine plus all loaded plugins.
pub struct PluginHost {
    engine: Engine,
    plugins: Vec<Plugin>,
}

impl PluginHost {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)?;
        Ok(Self {
            engine,
            plugins: Vec::new(),
        })
    }

    /// Number of loaded plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
    pub fn plugins(&self) -> impl Iterator<Item = &Plugin> {
        self.plugins.iter()
    }

    /// Load a plugin from wasm/WAT bytes and its manifest. Fails if the
    /// module imports host functions the manifest doesn't permit.
    pub fn load(&mut self, wasm: &[u8], manifest: PluginManifest) -> Result<()> {
        let name = manifest.name.clone();
        let module = Module::new(&self.engine, wasm)
            .map_err(anyhow::Error::from)
            .context("invalid plugin module")?;
        // Every `env.*` import must be covered by a declared permission.
        for import in module.imports() {
            if import.module() == "env" {
                let perm =
                    import_permission(import.name()).with_context(|| format!("plugin `{name}`"))?;
                if !manifest.permits(perm) {
                    bail!(
                        "plugin `{name}` uses `env.{}` without the `{perm}` permission",
                        import.name()
                    );
                }
            }
        }

        let limits = StoreLimitsBuilder::new()
            .memory_size(manifest.memory_max_bytes)
            .instances(8)
            .tables(8)
            .memories(2)
            .build();
        let mut store = Store::new(
            &self.engine,
            HostState {
                plugin: name.clone(),
                kv: PluginKv::new(),
                logs: Vec::new(),
                limits,
            },
        );
        store.limiter(|s| &mut s.limits);
        // Instantiation itself (e.g. data-segment init) consumes fuel.
        store
            .set_fuel(manifest.fuel_per_tick)
            .map_err(anyhow::Error::from)
            .context("fuel")?;

        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        link_imports(&mut linker, &manifest).with_context(|| format!("plugin `{name}`"))?;
        let instance = linker
            .instantiate(&mut store, &module)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("plugin `{name}` failed to instantiate"))?;
        self.plugins.push(Plugin {
            manifest,
            store,
            instance,
        });
        Ok(())
    }

    /// Run `vn_init` on every loaded plugin. `kv` is swapped in/out of
    /// each store so `vn_kv_*` imports see the runtime's shared state.
    pub fn init_all(&mut self, kv: &mut PluginKv) {
        self.call_all("vn_init", (), kv);
    }

    /// Run `vn_tick(dt)` on every loaded plugin.
    pub fn tick_all(&mut self, kv: &mut PluginKv, dt: f32) {
        self.call_all("vn_tick", (dt,), kv);
    }

    /// Notify plugins of a menu choice selection.
    pub fn notify_choice(&mut self, kv: &mut PluginKv, index: usize) {
        self.call_all("vn_on_choice", (index as i32,), kv);
    }

    /// Deliver a `plugin <name> ...` command. The JSON payload is copied
    /// into the plugin's memory via its `vn_alloc` export, then
    /// `vn_on_command(ptr, len)` runs within the fuel budget. Returns
    /// false if no such plugin is loaded or it doesn't handle commands.
    pub fn send_command(&mut self, kv: &mut PluginKv, name: &str, payload: &str) -> bool {
        let Some(plugin) = self.plugins.iter_mut().find(|p| p.name() == name) else {
            log::warn!("plugin command for unknown plugin `{name}`");
            return false;
        };
        std::mem::swap(&mut plugin.store.data_mut().kv, kv);
        let result = plugin.deliver(payload.as_bytes());
        for line in plugin.store.data_mut().logs.drain(..) {
            log::info!("plugin `{name}`: {line}");
        }
        std::mem::swap(&mut plugin.store.data_mut().kv, kv);
        match result {
            Ok(handled) => handled,
            Err(e) => {
                log::warn!("plugin `{name}` command error: {e:#}");
                false
            }
        }
    }

    /// All hooks return `()`, so `O` is fixed.
    fn call_all<I>(&mut self, func: &str, params: I, kv: &mut PluginKv)
    where
        I: wasmtime::WasmParams + Clone,
    {
        for plugin in &mut self.plugins {
            std::mem::swap(&mut plugin.store.data_mut().kv, kv);
            match plugin.call::<I, ()>(func, params.clone()) {
                Ok(_) => {
                    for line in plugin.store.data_mut().logs.drain(..) {
                        log::info!("plugin `{}`: {line}", plugin.manifest.name);
                    }
                }
                Err(e) => {
                    log::warn!("plugin `{}` error in {func}: {e}", plugin.manifest.name);
                    plugin.store.data_mut().logs.clear();
                }
            }
            std::mem::swap(&mut plugin.store.data_mut().kv, kv);
        }
    }
}

fn link_imports(linker: &mut Linker<HostState>, manifest: &PluginManifest) -> Result<()> {
    if manifest.permits("log") {
        linker.func_wrap(
            "env",
            "vn_log",
            |mut caller: Caller<HostState>, ptr: i32, len: i32| -> wasmtime::Result<()> {
                let bytes = read_mem(&mut caller, ptr, len).map_err(werr)?;
                let msg = String::from_utf8_lossy(&bytes).into_owned();
                caller.data_mut().logs.push(msg);
                Ok(())
            },
        )?;
    }
    if manifest.permits("kv") {
        linker.func_wrap(
            "env",
            "vn_kv_set",
            |mut caller: Caller<HostState>,
             kptr: i32,
             klen: i32,
             vptr: i32,
             vlen: i32|
             -> wasmtime::Result<()> {
                let key = read_mem(&mut caller, kptr, klen).map_err(werr)?;
                let value = read_mem(&mut caller, vptr, vlen).map_err(werr)?;
                let key = String::from_utf8_lossy(&key).into_owned();
                let value = String::from_utf8_lossy(&value).into_owned();
                let plugin = caller.data().plugin.clone();
                caller
                    .data_mut()
                    .kv
                    .insert(format!("{plugin}.{key}"), value);
                Ok(())
            },
        )?;
        linker.func_wrap(
            "env",
            "vn_kv_get",
            |mut caller: Caller<HostState>,
             kptr: i32,
             klen: i32,
             dptr: i32,
             dcap: i32|
             -> wasmtime::Result<i32> {
                let key = read_mem(&mut caller, kptr, klen).map_err(werr)?;
                let key = String::from_utf8_lossy(&key).into_owned();
                let plugin = caller.data().plugin.clone();
                let Some(value) = caller.data().kv.get(&format!("{plugin}.{key}")) else {
                    return Ok(-1);
                };
                let bytes: Vec<u8> =
                    value.as_bytes()[..value.len().min(dcap.max(0) as usize)].to_vec();
                let n = bytes.len() as i32;
                write_mem(&mut caller, dptr, &bytes).map_err(werr)?;
                Ok(n)
            },
        )?;
    }
    Ok(())
}

/// Host closures return `wasmtime::Error` (not `anyhow`), so map the
/// message over; a host-fn error traps the calling plugin.
fn werr(e: anyhow::Error) -> wasmtime::Error {
    wasmtime::Error::msg(e.to_string())
}

/// Read `len` bytes of plugin memory at `ptr`.
fn read_mem(caller: &mut Caller<HostState>, ptr: i32, len: i32) -> Result<Vec<u8>> {
    let mem = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .context("plugin exports no `memory`")?;
    if len < 0 {
        bail!("negative length {len}");
    }
    let mut buf = vec![0u8; len as usize];
    mem.read(&caller, ptr as usize, &mut buf)
        .context("memory read out of bounds")?;
    Ok(buf)
}

fn write_mem(caller: &mut Caller<HostState>, ptr: i32, bytes: &[u8]) -> Result<()> {
    let mem = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .context("plugin exports no `memory`")?;
    mem.write(&mut *caller, ptr as usize, bytes)
        .context("memory write out of bounds")?;
    Ok(())
}

/// Load `*.wasm` + `*.toml` manifest pairs found under `prefix`
/// (e.g. `plugins`) in any [`AssetSource`] — works for loose project
/// directories and `.vnpkg` packages alike. Rejected plugins are logged
/// and skipped, never fatal.
pub fn load_source(
    host: &mut PluginHost,
    source: &dyn vnengine_core::assets::AssetSource,
    prefix: &str,
) {
    for path in source.list(prefix) {
        // `.wasm` binaries and `.wat` text modules (dev-friendly) both
        // load — wasmtime's `wat` feature detects text format.
        let ext = path.rsplit('.').next().unwrap_or("");
        if ext != "wasm" && ext != "wat" {
            continue;
        }
        let stem = path
            .rsplit('/')
            .next()
            .and_then(|f| f.rsplit_once('.').map(|(s, _)| s))
            .unwrap_or_default()
            .to_string();
        if let Err(e) = load_entry(host, source, &path, &stem) {
            log::warn!("plugin `{stem}` rejected: {e:#}");
        }
    }
}

fn load_entry(
    host: &mut PluginHost,
    source: &dyn vnengine_core::assets::AssetSource,
    wasm_path: &str,
    stem: &str,
) -> Result<()> {
    let wasm = source.read(wasm_path)?;
    let base = wasm_path
        .rsplit_once('.')
        .map(|(b, _)| b)
        .unwrap_or(wasm_path);
    let manifest_src = String::from_utf8(
        source
            .read(&format!("{base}.toml"))
            .context("missing `<name>.toml` manifest")?,
    )?;
    let manifest = PluginManifest::for_file(&manifest_src, stem)?;
    host.load(&wasm, manifest)
}

#[cfg(test)]
mod tests;
