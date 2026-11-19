//! Plugin auto-converter: turn external tools into KimiOS plugins without
//! hand-authoring manifests. Sources:
//! - Alpine packages (`import_alpine`): install into the sysroot via
//!   [`alpine::install`], detect the primary executable, wrap it as a native
//!   plugin whose entry uses the `sysroot:` prefix.
//! - Raw Python scripts (`import_python_url`): fetch a .py and wrap it.
//! - Pre-installed npm packages (`import_npm`): wrap an existing
//!   `/disk/import/npm-<pkg>/node_modules/<pkg>` tree (the npm install itself
//!   is triggered elsewhere).
//!
//! Staging happens under `/disk/import/.<kind>-<name>/`; [`kplugin::install`]
//! copies the tree to `/disk/plugins/<id>/` and enables it. Staging is removed
//! afterwards (and on any failure).
//!
//! `sysroot:` entry convention: the host plugin loader resolves
//! `sysroot:<path>` as `/disk/alpine/root/<path>`, so Alpine binaries keep
//! their FHS data paths (share/, lib/, etc.) intact instead of being copied
//! into the plugin dir.

use crate as k;
use crate::{alpine, kplugin};

const IMPORT_DIR: &str = "/disk/import";
const SYSROOT: &str = "/disk/alpine/root";

// -- small shared helpers (fixed buffers, no allocation) ---------------------

fn push(buf: &mut [u8], n: &mut usize, data: &[u8]) {
    let take = data.len().min(buf.len() - *n);
    buf[*n..*n + take].copy_from_slice(&data[..take]);
    *n += take;
}

fn build_path(buf: &mut [u8], parts: &[&str]) -> usize {
    let mut n = 0usize;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 && n < buf.len() { buf[n] = b'/'; n += 1; }
        push(buf, &mut n, p.as_bytes());
    }
    n
}

fn path_str(buf: &[u8], n: usize) -> &str {
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

/// Copy `src` into `dst` truncated to `dst.len()`, dropping quotes and
/// backslashes, mapping whitespace/control bytes to spaces and stripping
/// non-ASCII, so the result is safe inside a JSON string value.
fn sanitize(src: &[u8], dst: &mut [u8]) -> usize {
    let mut n = 0usize;
    for &b in src {
        if n >= dst.len() { break; }
        if b == b'"' || b == b'\\' { continue; }
        let c = if b < 0x20 { b' ' } else if b >= 0x80 { continue } else { b };
        dst[n] = c;
        n += 1;
    }
    n
}

/// Build a plugin id: `prefix` + sanitized `src`, capped at `dst.len()` (48).
/// Keeps [A-Za-z0-9._-], maps everything else to '_', never emits '/' or "..".
fn make_id(prefix: &str, src: &str, dst: &mut [u8]) -> usize {
    let mut n = 0usize;
    push(dst, &mut n, prefix.as_bytes());
    let mut prev_dot = prefix.ends_with('.');
    for &b in src.as_bytes() {
        if n >= dst.len() { break; }
        let c = if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            b
        } else if b == b'.' && !prev_dot {
            b'.'
        } else {
            b'_'
        };
        dst[n] = c;
        n += 1;
        prev_dot = c == b'.';
    }
    n
}

fn mkdir_p(path: &str) {
    let mut acc = [0u8; 256];
    let mut n = 0usize;
    for part in path.split('/') {
        if part.is_empty() || part == "." { continue; }
        if n == 0 && path.starts_with('/') { acc[0] = b'/'; n = 1; }
        if n + part.len() + 1 >= acc.len() { return; }
        push(&mut acc, &mut n, part.as_bytes());
        let _ = k::mkdir(path_str(&acc, n));
        acc[n] = b'/';
        n += 1;
    }
}

fn remove_tree(path: &str) {
    let mut buf = [0u8; 2048];
    if let Some(n) = k::listdir(path, &mut buf) {
        for entry in buf[..n].split(|&b| b == 0) {
            if entry.is_empty() || entry == b"." || entry == b".." { continue; }
            let is_dir = entry.ends_with(b"/");
            let name = if is_dir { &entry[..entry.len() - 1] } else { entry };
            let Ok(namestr) = core::str::from_utf8(name) else { continue };
            let mut cp = [0u8; 192];
            let cn = build_path(&mut cp, &[path, namestr]);
            let cp = path_str(&cp, cn);
            if is_dir { remove_tree(cp); let _ = k::rmdir(cp); } else { let _ = k::unlink(cp); }
        }
    }
    let _ = k::rmdir(path);
}

fn copy_tree(src: &str, dst: &str) -> bool {
    mkdir_p(dst);
    let mut buf = [0u8; 2048];
    let Some(n) = k::listdir(src, &mut buf) else { return false };
    for entry in buf[..n].split(|&b| b == 0) {
        if entry.is_empty() || entry == b"." || entry == b".." { continue; }
        let is_dir = entry.ends_with(b"/");
        let name = if is_dir { &entry[..entry.len() - 1] } else { entry };
        let Ok(namestr) = core::str::from_utf8(name) else { continue };
        let mut sp = [0u8; 192]; let sn = build_path(&mut sp, &[src, namestr]);
        let mut dp = [0u8; 192]; let dn = build_path(&mut dp, &[dst, namestr]);
        let sp = path_str(&sp, sn);
        let dp = path_str(&dp, dn);
        if is_dir {
            if !copy_tree(sp, dp) { return false; }
        } else if !k::copy_file(sp, dp) {
            return false;
        }
    }
    true
}

/// Write the plugin manifest into `dir/plugin.json`. All string values are
/// sanitized; the compact layout stays well under the 2 KiB manifest cap.
fn write_manifest(
    dir: &str,
    id: &[u8],
    name: &[u8],
    version: &[u8],
    runtime: &str,
    entry: &[u8],
    desc: &[u8],
) -> bool {
    let mut sid = [0u8; 48]; let idl = sanitize(id, &mut sid);
    let mut sname = [0u8; 48]; let namel = sanitize(name, &mut sname);
    let mut sver = [0u8; 16]; let verl = sanitize(version, &mut sver);
    let mut sentry = [0u8; 160]; let entryl = sanitize(entry, &mut sentry);
    let mut sdesc = [0u8; 200]; let descl = sanitize(desc, &mut sdesc);
    if idl == 0 || entryl == 0 { return false; }
    let mut j = [0u8; 1024];
    let mut n = 0usize;
    push(&mut j, &mut n, b"{\"id\":\"");
    push(&mut j, &mut n, &sid[..idl]);
    push(&mut j, &mut n, b"\",\"name\":\"");
    push(&mut j, &mut n, &sname[..namel]);
    push(&mut j, &mut n, b"\",\"version\":\"");
    push(&mut j, &mut n, &sver[..verl]);
    push(&mut j, &mut n, b"\",\"type\":\"app\",\"runtime\":\"");
    push(&mut j, &mut n, runtime.as_bytes());
    push(&mut j, &mut n, b"\",\"entry\":\"");
    push(&mut j, &mut n, &sentry[..entryl]);
    push(&mut j, &mut n, b"\",\"args\":[],\"permissions\":[],\"description\":\"");
    push(&mut j, &mut n, &sdesc[..descl]);
    push(&mut j, &mut n, b"\"}");
    let mut mp = [0u8; 192];
    let mn = build_path(&mut mp, &[dir, "plugin.json"]);
    let Some(h) = k::create(path_str(&mp, mn)) else { return false };
    let wrote = k::write_file(h, 0, &j[..n]);
    k::close(h);
    wrote == n
}

/// Prepare a fresh staging dir `/disk/import/.<kind>-<token>`.
fn staging(kind: &[u8], token: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut sname = [0u8; 64];
    let mut sn = 0usize;
    push(&mut sname, &mut sn, b".");
    push(&mut sname, &mut sn, kind);
    push(&mut sname, &mut sn, b"-");
    push(&mut sname, &mut sn, token);
    let sname = path_str(&sname, sn);
    let dn = build_path(out, &[IMPORT_DIR, sname]);
    let dir = path_str(out, dn);
    let _ = k::mkdir(IMPORT_DIR);
    remove_tree(dir);
    if !k::mkdir(dir) { return None; }
    Some(dn)
}

// -- 1. Alpine package -> native plugin --------------------------------------

/// Try `SYSROOT/<dir>/<name>`; on success write the `sysroot:<dir>/<name>`
/// entry into `entry` and return its length.
fn try_binary(dir: &str, name: &[u8], entry: &mut [u8]) -> Option<usize> {
    if name.is_empty() || name.len() > 64 || name.contains(&b'/') || name.windows(2).any(|w| w == b"..") {
        return None;
    }
    let mut p = [0u8; 160];
    let mut n = 0usize;
    push(&mut p, &mut n, SYSROOT.as_bytes());
    push(&mut p, &mut n, b"/");
    push(&mut p, &mut n, dir.as_bytes());
    push(&mut p, &mut n, b"/");
    push(&mut p, &mut n, name);
    if k::stat(path_str(&p, n)).is_none() { return None; }
    let mut m = 0usize;
    push(entry, &mut m, b"sysroot:");
    push(entry, &mut m, dir.as_bytes());
    push(entry, &mut m, b"/");
    push(entry, &mut m, name);
    (m > 8).then_some(m)
}

/// Detect the package's primary executable in the installed sysroot.
/// Candidate order: the `cmd:` token matching the package name, then the other
/// provided commands, then the package name itself; each is looked up in
/// `usr/bin/` then `bin/` via stat. Multi-binary packages wrap only the primary.
fn find_binary(pkg: &str, commands: &[u8], entry: &mut [u8]) -> Option<usize> {
    let mut cands: [[u8; 64]; 8] = [[0; 64]; 8];
    let mut clens = [0usize; 8];
    let mut cn = 0usize;
    let pb = pkg.as_bytes();
    for tok in commands.split(|&b| b == b' ') {
        if tok == pb && cn < cands.len() {
            clens[cn] = tok.len().min(64);
            cands[cn][..clens[cn]].copy_from_slice(&tok[..clens[cn]]);
            cn += 1;
            break;
        }
    }
    for tok in commands.split(|&b| b == b' ') {
        if tok.is_empty() || tok == pb || cn >= cands.len() { continue; }
        clens[cn] = tok.len().min(64);
        cands[cn][..clens[cn]].copy_from_slice(&tok[..clens[cn]]);
        cn += 1;
    }
    if cn < cands.len() && pb.len() <= 64 {
        clens[cn] = pb.len();
        cands[cn][..pb.len()].copy_from_slice(pb);
        cn += 1;
    }
    for i in 0..cn {
        let name = &cands[i][..clens[i]];
        if let Some(l) = try_binary("usr/bin", name, entry) { return Some(l); }
        if let Some(l) = try_binary("bin", name, entry) { return Some(l); }
    }
    None
}

/// Convert an Alpine package into an installed, enabled native plugin.
/// Installs the package + deps into the sysroot, detects its primary binary,
/// writes a staging manifest with a `sysroot:` entry and registers it via
/// kplugin::install. Returns false (cleaning up staging) on any failure.
pub fn import_alpine(pkg: &str) -> bool {
    if pkg.is_empty() || pkg.len() > 44 || pkg.contains('/') || pkg.contains("..") {
        return false;
    }
    if !alpine::install(pkg) { return false; }
    let meta = alpine::package_info(pkg);

    let mut token = [0u8; 48];
    let tl = make_id("", pkg, &mut token);
    let mut dirb = [0u8; 160];
    let Some(dn) = staging(b"alp", &token[..tl], &mut dirb) else { return false };
    let dir = path_str(&dirb, dn);

    let commands: &[u8] = meta.as_ref().map(|m| &m.commands[..m.commands_len]).unwrap_or(b"");
    let mut entry = [0u8; 160];
    let Some(el) = find_binary(pkg, commands, &mut entry) else {
        k::print("kimport: no executable found for ");
        k::print(pkg);
        k::print("\n");
        remove_tree(dir);
        return false;
    };

    let mut id = [0u8; 48];
    let idl = make_id("alp.", pkg, &mut id);
    let version: &[u8] = meta.as_ref().filter(|m| m.version_len > 0).map(|m| &m.version[..m.version_len]).unwrap_or(b"0");
    let mut desc = [0u8; 200];
    let mut dlen = 0usize;
    let t: &[u8] = meta.as_ref().filter(|m| m.desc_len > 0).map(|m| &m.desc[..m.desc_len]).unwrap_or(pkg.as_bytes());
    dlen += sanitize(t, &mut desc);
    push(&mut desc, &mut dlen, b" (Alpine");
    if let Some(m) = &meta {
        if m.license_len > 0 {
            push(&mut desc, &mut dlen, b", ");
            let mut lic = [0u8; 24];
            let ll = sanitize(&m.license[..m.license_len], &mut lic);
            push(&mut desc, &mut dlen, &lic[..ll]);
        }
    }
    push(&mut desc, &mut dlen, b")");

    let ok = write_manifest(dir, &id[..idl], pkg.as_bytes(), version, "native", &entry[..el], &desc[..dlen])
        && kplugin::install(dir);
    remove_tree(dir);
    if ok {
        k::print("kimport: plugin alp.");
        k::print(pkg);
        k::print(" installed\n");
    }
    ok
}

// -- 2. raw Python script -> python plugin ------------------------------------

/// Fetch a raw .py from `url` and wrap it as a python plugin `py.<id>`.
/// Stdlib gating is handled by the curated catalog; this just wraps.
pub fn import_python_url(url: &str, id: &str, name: &str) -> bool {
    if url.is_empty() || id.is_empty() || id.len() > 44 { return false; }
    let mut token = [0u8; 48];
    let tl = make_id("", id, &mut token);
    if tl == 0 { return false; }
    let mut dirb = [0u8; 160];
    let Some(dn) = staging(b"py", &token[..tl], &mut dirb) else { return false };
    let dir = path_str(&dirb, dn);

    let mut dest = [0u8; 192];
    let dn2 = build_path(&mut dest, &[dir, "main.py"]);
    let fetched = k::fetch_url(url, path_str(&dest, dn2));
    if !matches!(fetched, Some(n) if n > 0) {
        remove_tree(dir);
        return false;
    }

    let mut pid = [0u8; 48];
    let idl = make_id("py.", id, &mut pid);
    let mut pname = [0u8; 48];
    let mut nl = sanitize(name.as_bytes(), &mut pname);
    if nl == 0 { nl = sanitize(id.as_bytes(), &mut pname); }
    let mut desc = [0u8; 200];
    let mut dlen = 0usize;
    push(&mut desc, &mut dlen, b"Python script from ");
    let mut surl = [0u8; 176];
    let ul = sanitize(url.as_bytes(), &mut surl);
    push(&mut desc, &mut dlen, &surl[..ul]);

    let ok = write_manifest(dir, &pid[..idl], &pname[..nl], b"1.0", "python", b"main.py", &desc[..dlen])
        && kplugin::install(dir);
    remove_tree(dir);
    ok
}

// -- 3. pre-installed npm package -> node plugin ------------------------------

fn skip_ws(data: &[u8], pos: &mut usize) {
    while *pos < data.len() && matches!(data[*pos], b' ' | b'\t' | b'\n' | b'\r') {
        *pos += 1;
    }
}

/// Read a JSON string value starting at `data[*pos] == '"'`; advances past the
/// closing quote. No escape processing (npm metadata fields are plain).
fn json_string<'a>(data: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    if *pos >= data.len() || data[*pos] != b'"' { return None; }
    let s = *pos + 1;
    let e = s + data[s..].iter().position(|&b| b == b'"')?;
    *pos = e + 1;
    Some(&data[s..e])
}

fn json_str<'a>(data: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let kb = key.as_bytes();
    let mut pat = [0u8; 40];
    if kb.len() + 2 > pat.len() { return None; }
    pat[0] = b'"';
    pat[1..1 + kb.len()].copy_from_slice(kb);
    pat[1 + kb.len()] = b'"';
    let plen = kb.len() + 2;
    let pos = data.windows(plen).position(|w| w == &pat[..plen])?;
    let mut p = pos + plen;
    skip_ws(data, &mut p);
    if data.get(p) != Some(&b':') { return None; }
    p += 1;
    skip_ws(data, &mut p);
    json_string(data, &mut p)
}

/// Extract the executable path from package.json's `bin` field, which may be
/// a string or an object. For objects prefer the key matching the package
/// name (or its tail after '/'), else the first entry.
fn json_bin<'a>(data: &'a [u8], pkg: &str) -> Option<&'a [u8]> {
    let pos = data.windows(5).position(|w| w == b"\"bin\"")?;
    let mut p = pos + 5;
    skip_ws(data, &mut p);
    if data.get(p) != Some(&b':') { return None; }
    p += 1;
    skip_ws(data, &mut p);
    match data.get(p)? {
        b'"' => json_string(data, &mut p),
        b'{' => {
            p += 1;
            let tail = pkg.rsplit('/').next().unwrap_or(pkg);
            let mut first: Option<&'a [u8]> = None;
            for _ in 0..64 {
                skip_ws(data, &mut p);
                match data.get(p) {
                    Some(b'}') | None => break,
                    Some(b'"') => {}
                    _ => return None,
                }
                let key = json_string(data, &mut p)?;
                skip_ws(data, &mut p);
                if data.get(p) != Some(&b':') { return None; }
                p += 1;
                skip_ws(data, &mut p);
                let val = json_string(data, &mut p)?;
                skip_ws(data, &mut p);
                if data.get(p) == Some(&b',') { p += 1; }
                let Ok(keys) = core::str::from_utf8(key) else { continue };
                if keys == pkg || keys == tail { return Some(val); }
                if first.is_none() { first = Some(val); }
            }
            first
        }
        _ => None,
    }
}

/// Wrap an already-installed npm package at
/// `/disk/import/npm-<pkg>/node_modules/<pkg>` as a node plugin `npm.<pkg>`.
/// Reads its package.json (bin/version/license/description), copies the
/// package dir into the plugin and registers it. The npm install itself is
/// triggered elsewhere (host hook); this is the wrap-only v1.
pub fn import_npm(pkg: &str) -> bool {
    if pkg.is_empty() || pkg.len() > 64 || pkg.contains("..") { return false; }
    let mut ndir = [0u8; 96];
    let mut nn = 0usize;
    push(&mut ndir, &mut nn, b"npm-");
    push(&mut ndir, &mut nn, pkg.as_bytes());
    let ndir = path_str(&ndir, nn);
    let mut srcb = [0u8; 192];
    let sn = build_path(&mut srcb, &[IMPORT_DIR, ndir, "node_modules", pkg]);
    let src = path_str(&srcb, sn);

    let mut mp = [0u8; 192];
    let mn = build_path(&mut mp, &[src, "package.json"]);
    let mut pj = [0u8; 2048];
    let Some(h) = k::open(path_str(&mp, mn)) else { return false };
    let pn = k::read_file(h, 0, &mut pj);
    k::close(h);
    if pn == 0 { return false; }
    let data = &pj[..pn];

    let Some(bin) = json_bin(data, pkg) else { return false };
    let bin = bin.strip_prefix(b"./").unwrap_or(bin);
    if bin.is_empty() || bin.len() > 96 || bin.starts_with(b"/") || bin.windows(2).any(|w| w == b"..") {
        return false;
    }
    let mut bp = [0u8; 192];
    let bn = build_path(&mut bp, &[src, path_str(bin, bin.len())]);
    if k::stat(path_str(&bp, bn)).is_none() { return false; }
    if 13 + pkg.len() + bin.len() > 160 { return false; }

    let mut token = [0u8; 48];
    let tl = make_id("", pkg, &mut token);
    let mut dirb = [0u8; 160];
    let Some(dn) = staging(b"npm", &token[..tl], &mut dirb) else { return false };
    let dir = path_str(&dirb, dn);

    let mut dest = [0u8; 192];
    let dlen = build_path(&mut dest, &[dir, "node_modules", pkg]);
    if !copy_tree(src, path_str(&dest, dlen)) {
        remove_tree(dir);
        return false;
    }

    let mut id = [0u8; 48];
    let idl = make_id("npm.", pkg, &mut id);
    let version = json_str(data, "version").filter(|v| !v.is_empty()).unwrap_or(b"1.0");
    let mut entry = [0u8; 160];
    let mut el = 0usize;
    push(&mut entry, &mut el, b"node_modules/");
    push(&mut entry, &mut el, pkg.as_bytes());
    push(&mut entry, &mut el, b"/");
    push(&mut entry, &mut el, bin);
    let mut desc = [0u8; 200];
    let mut dl = 0usize;
    let t = json_str(data, "description").filter(|d| !d.is_empty()).unwrap_or(pkg.as_bytes());
    dl += sanitize(t, &mut desc);
    push(&mut desc, &mut dl, b" (npm");
    if let Some(lic) = json_str(data, "license").filter(|l| !l.is_empty()) {
        push(&mut desc, &mut dl, b", ");
        let mut licb = [0u8; 24];
        let ll = sanitize(lic, &mut licb);
        push(&mut desc, &mut dl, &licb[..ll]);
    }
    push(&mut desc, &mut dl, b")");

    let ok = write_manifest(dir, &id[..idl], pkg.as_bytes(), version, "node", &entry[..el], &desc[..dl])
        && kplugin::install(dir);
    remove_tree(dir);
    ok
}
