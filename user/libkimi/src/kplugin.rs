//! Plugin system manager (native guest plugins). Plugins are packages installed
//! to `/disk/plugins/<id>/` with a `plugin.json` manifest; enable state lives in
//! `/disk/plugins.cfg` (survives restart). Execution goes through the host guest
//! loader (`SYS_PLUGIN_RUN`), which builds argv from the manifest and injects the
//! Plugin API environment. See docs/plugin-system.md.

use crate as k;

const PLUGIN_DIR: &str = "/disk/plugins";
const REGISTRY: &str = "/disk/plugins.cfg";

pub const TYPE_APP: u8 = 0;
pub const TYPE_SERVICE: u8 = 1;
pub const TYPE_UI: u8 = 2;

#[derive(Clone, Copy)]
pub struct PluginInfo {
    pub id: [u8; 48],
    pub id_len: usize,
    pub name: [u8; 48],
    pub name_len: usize,
    pub version: [u8; 16],
    pub version_len: usize,
    pub ptype: u8,
    pub runtime: u8,
    pub enabled: bool,
}

impl PluginInfo {
    pub fn id_str(&self) -> &str { core::str::from_utf8(&self.id[..self.id_len]).unwrap_or("") }
    pub fn name_str(&self) -> &str { core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("") }
    pub fn version_str(&self) -> &str { core::str::from_utf8(&self.version[..self.version_len]).unwrap_or("") }
    pub fn type_str(&self) -> &str {
        match self.ptype { TYPE_SERVICE => "service", TYPE_UI => "ui", _ => "app" }
    }
}

fn empty_info() -> PluginInfo {
    PluginInfo {
        id: [0; 48], id_len: 0, name: [0; 48], name_len: 0,
        version: [0; 16], version_len: 0, ptype: TYPE_APP, runtime: 0, enabled: false,
    }
}

// -- tiny JSON string extractor (compact manifest) --------------------------
fn json_str<'a>(data: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let kb = key.as_bytes();
    let mut pat = [0u8; 40];
    if kb.len() + 3 > pat.len() { return None; }
    pat[0] = b'"';
    pat[1..1 + kb.len()].copy_from_slice(kb);
    let mut n = 1 + kb.len();
    pat[n] = b'"'; n += 1;
    pat[n] = b':'; n += 1;
    let pos = data.windows(n).position(|w| w == &pat[..n])?;
    let mut s = pos + n;
    while s < data.len() && matches!(data[s], b' ' | b'\t' | b'\n' | b'\r') { s += 1; }
    if s >= data.len() || data[s] != b'"' { return None; }
    s += 1;
    let e = s + data[s..].iter().position(|&b| b == b'"')?;
    Some(&data[s..e])
}

fn copy_field(dst: &mut [u8], len: &mut usize, src: Option<&[u8]>) {
    if let Some(s) = src {
        let n = s.len().min(dst.len());
        dst[..n].copy_from_slice(&s[..n]);
        *len = n;
    }
}

fn read_file_all(path: &str, buf: &mut [u8]) -> Option<usize> {
    let h = k::open(path)?;
    let n = k::read_file(h, 0, buf);
    k::close(h);
    Some(n)
}

fn parse_manifest(data: &[u8], info: &mut PluginInfo) {
    copy_field(&mut info.id, &mut info.id_len, json_str(data, "id"));
    copy_field(&mut info.name, &mut info.name_len, json_str(data, "name"));
    copy_field(&mut info.version, &mut info.version_len, json_str(data, "version"));
    info.ptype = match json_str(data, "type") {
        Some(b"service") => TYPE_SERVICE,
        Some(b"ui") => TYPE_UI,
        _ => TYPE_APP,
    };
    info.runtime = match json_str(data, "runtime") {
        Some(b"python") => 1,
        Some(b"node") => 2,
        _ => 0,
    };
}

// -- registry (lines: "<id> <0|1>") -----------------------------------------
fn registry_enabled(id: &str) -> bool {
    let mut buf = [0u8; 4096];
    let Some(n) = read_file_all(REGISTRY, &mut buf) else { return true };
    for line in buf[..n].split(|&b| b == b'\n') {
        let Some(sp) = line.iter().position(|&b| b == b' ') else { continue };
        if &line[..sp] == id.as_bytes() {
            return line.get(sp + 1) == Some(&b'1');
        }
    }
    true // default enabled
}

fn registry_set(id: &str, enabled: bool) {
    let mut buf = [0u8; 4096];
    let n = read_file_all(REGISTRY, &mut buf).unwrap_or(0);
    let mut out = [0u8; 4096];
    let mut used = 0usize;
    let mut found = false;
    for line in buf[..n].split(|&b| b == b'\n') {
        if line.is_empty() { continue; }
        let sp = line.iter().position(|&b| b == b' ');
        let lid = if let Some(s) = sp { &line[..s] } else { line };
        let en = if lid == id.as_bytes() { found = true; enabled } else {
            sp.and_then(|s| line.get(s + 1)).map_or(true, |&b| b == b'1')
        };
        let entry_len = lid.len() + 2;
        if used + entry_len >= out.len() { continue; }
        out[used..used + lid.len()].copy_from_slice(lid);
        used += lid.len();
        out[used] = b' '; used += 1;
        out[used] = if en { b'1' } else { b'0' }; used += 1;
        out[used] = b'\n'; used += 1;
    }
    if !found {
        let idb = id.as_bytes();
        if used + idb.len() + 2 < out.len() {
            out[used..used + idb.len()].copy_from_slice(idb);
            used += idb.len();
            out[used] = b' '; used += 1;
            out[used] = if enabled { b'1' } else { b'0' }; used += 1;
            out[used] = b'\n'; used += 1;
        }
    }
    if let Some(h) = k::create(REGISTRY) {
        k::write_file(h, 0, &out[..used]);
        k::close(h);
    }
}

fn registry_remove(id: &str) {
    let mut buf = [0u8; 4096];
    let n = read_file_all(REGISTRY, &mut buf).unwrap_or(0);
    let mut out = [0u8; 4096];
    let mut used = 0usize;
    for line in buf[..n].split(|&b| b == b'\n') {
        if line.is_empty() { continue; }
        let sp = line.iter().position(|&b| b == b' ');
        let lid = if let Some(s) = sp { &line[..s] } else { line };
        if lid == id.as_bytes() { continue; }
        if used + line.len() + 1 >= out.len() { continue; }
        out[used..used + line.len()].copy_from_slice(line);
        used += line.len();
        out[used] = b'\n'; used += 1;
    }
    if let Some(h) = k::create(REGISTRY) {
        k::write_file(h, 0, &out[..used]);
        k::close(h);
    }
}

// -- public API -------------------------------------------------------------
/// List installed plugins (scans /disk/plugins/*/plugin.json). Returns count.
pub fn list(out: &mut [PluginInfo]) -> usize {
    let mut buf = [0u8; 2048];
    let Some(n) = k::listdir(PLUGIN_DIR, &mut buf) else { return 0 };
    let mut count = 0usize;
    for entry in buf[..n].split(|&b| b == 0) {
        if entry.is_empty() || !entry.ends_with(b"/") || count >= out.len() { continue; }
        let id = &entry[..entry.len() - 1];
        let Ok(idstr) = core::str::from_utf8(id) else { continue };
        let mut path = [0u8; 160];
        let pn = build_path(&mut path, &[PLUGIN_DIR, idstr, "plugin.json"]);
        let mpath = core::str::from_utf8(&path[..pn]).unwrap_or("");
        let mut mbuf = [0u8; 2048];
        let Some(mn) = read_file_all(mpath, &mut mbuf) else { continue };
        let mut info = empty_info();
        parse_manifest(&mbuf[..mn], &mut info);
        if info.id_len == 0 {
            copy_field(&mut info.id, &mut info.id_len, Some(id));
        }
        if info.name_len == 0 {
            copy_field(&mut info.name, &mut info.name_len, Some(id));
        }
        info.enabled = registry_enabled(info.id_str());
        out[count] = info;
        count += 1;
    }
    count
}

fn build_path(buf: &mut [u8], parts: &[&str]) -> usize {
    let mut n = 0usize;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 && n < buf.len() { buf[n] = b'/'; n += 1; }
        let pb = p.as_bytes();
        let take = pb.len().min(buf.len() - n);
        buf[n..n + take].copy_from_slice(&pb[..take]);
        n += take;
    }
    n
}

/// Install a plugin from a source directory (containing plugin.json) under
/// /disk. Copies the tree to /disk/plugins/<id> and enables it. Returns true.
pub fn install(src_dir: &str) -> bool {
    let mut mpath = [0u8; 160];
    let mn = build_path(&mut mpath, &[src_dir, "plugin.json"]);
    let mpath = core::str::from_utf8(&mpath[..mn]).unwrap_or("");
    let mut mbuf = [0u8; 2048];
    let Some(n) = read_file_all(mpath, &mut mbuf) else { return false };
    let mut info = empty_info();
    parse_manifest(&mbuf[..n], &mut info);
    if info.id_len == 0 { return false; }
    let id = info.id_str();
    if id.contains("..") || id.contains('/') { return false; }
    let mut dst = [0u8; 128];
    let dn = build_path(&mut dst, &[PLUGIN_DIR, id]);
    let dst = core::str::from_utf8(&dst[..dn]).unwrap_or("");
    let _ = k::mkdir(PLUGIN_DIR);
    if !copy_tree(src_dir, dst) { return false; }
    registry_set(id, true);
    true
}

fn copy_tree(src: &str, dst: &str) -> bool {
    let _ = k::mkdir(dst);
    let mut buf = [0u8; 2048];
    let Some(n) = k::listdir(src, &mut buf) else { return false };
    for entry in buf[..n].split(|&b| b == 0) {
        if entry.is_empty() || entry == b"." || entry == b".." { continue; }
        let is_dir = entry.ends_with(b"/");
        let name = if is_dir { &entry[..entry.len() - 1] } else { entry };
        let Ok(namestr) = core::str::from_utf8(name) else { continue };
        let mut sp = [0u8; 160]; let sn = build_path(&mut sp, &[src, namestr]);
        let mut dp = [0u8; 160]; let dn = build_path(&mut dp, &[dst, namestr]);
        let sp = core::str::from_utf8(&sp[..sn]).unwrap_or("");
        let dp = core::str::from_utf8(&dp[..dn]).unwrap_or("");
        if is_dir {
            if !copy_tree(sp, dp) { return false; }
        } else if !k::copy_file(sp, dp) {
            return false;
        }
    }
    true
}

fn remove_tree(path: &str) {
    let mut buf = [0u8; 2048];
    if let Some(n) = k::listdir(path, &mut buf) {
        for entry in buf[..n].split(|&b| b == 0) {
            if entry.is_empty() || entry == b"." || entry == b".." { continue; }
            let is_dir = entry.ends_with(b"/");
            let name = if is_dir { &entry[..entry.len() - 1] } else { entry };
            let Ok(namestr) = core::str::from_utf8(name) else { continue };
            let mut cp = [0u8; 160]; let cn = build_path(&mut cp, &[path, namestr]);
            let cp = core::str::from_utf8(&cp[..cn]).unwrap_or("");
            if is_dir { remove_tree(cp); let _ = k::rmdir(cp); } else { let _ = k::unlink(cp); }
        }
    }
    let _ = k::rmdir(path);
}

pub fn set_enabled(id: &str, enabled: bool) -> bool {
    registry_set(id, enabled);
    true
}

pub fn uninstall(id: &str) -> bool {
    if id.contains("..") || id.contains('/') { return false; }
    let mut path = [0u8; 128];
    let n = build_path(&mut path, &[PLUGIN_DIR, id]);
    let path = core::str::from_utf8(&path[..n]).unwrap_or("");
    remove_tree(path);
    registry_remove(id);
    true
}

/// Run an app plugin (foreground). Returns exit code.
pub fn run(id: &str) -> Option<u64> {
    k::plugin_run(id)
}

/// Start all enabled service plugins in the background.
pub fn start_services() {
    let mut infos = [empty_info(); 32];
    let n = list(&mut infos);
    for i in 0..n {
        if infos[i].ptype == TYPE_SERVICE && infos[i].enabled {
            let _ = k::plugin_run_bg(infos[i].id_str());
        }
    }
}

/// Apply a UI plugin's bundled theme/font (type=ui). Returns true if applied.
pub fn apply_ui(id: &str) -> bool {
    let mut path = [0u8; 160];
    let n = build_path(&mut path, &[PLUGIN_DIR, id, "plugin.json"]);
    let path = core::str::from_utf8(&path[..n]).unwrap_or("");
    let mut buf = [0u8; 2048];
    let Some(n) = read_file_all(path, &mut buf) else { return false };
    let data = &buf[..n];
    let mut applied = false;
    if let Some(theme) = json_str(data, "theme") {
        if let Ok(t) = core::str::from_utf8(theme) {
            let mut tp = [0u8; 160];
            let tn = build_path(&mut tp, &[PLUGIN_DIR, id, t]);
            let tp = core::str::from_utf8(&tp[..tn]).unwrap_or("");
            if k::stat(tp).is_some() { applied = true; }
        }
    }
    applied
}

pub fn api_port() -> u16 {
    k::plugin_api_port()
}
