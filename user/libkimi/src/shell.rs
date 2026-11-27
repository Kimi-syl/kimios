//! Shared command interpreter used by both the init CLI shell and the GUI
//! Terminal app, so the two accept exactly the same commands. Output goes
//! through `crate::print*`, which the GUI can redirect into its display via
//! the console-capture hook (`capture_begin`/`capture_take`/`capture_end`).

use crate as k;

fn trim(input: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = input.len();

    while start < end && (input[start] == b' ' || input[start] == b'\t') {
        start += 1;
    }
    while end > start
        && (input[end - 1] == b' ' || input[end - 1] == b'\t' || input[end - 1] == b'\r')
    {
        end -= 1;
    }

    &input[start..end]
}

fn split_word(input: &[u8]) -> (&[u8], &[u8]) {
    let mut index = 0;
    while index < input.len() && input[index] != b' ' && input[index] != b'\t' {
        index += 1;
    }
    let word = &input[..index];

    let mut rest = index;
    while rest < input.len() && (input[rest] == b' ' || input[rest] == b'\t') {
        rest += 1;
    }

    (word, &input[rest..])
}

/// Accept both `hello` and `/hello`.
fn with_slash<'a>(argument: &[u8], buffer: &'a mut [u8]) -> Option<&'a str> {
    if argument.is_empty() {
        return None;
    }

    let needs_slash = argument[0] != b'/';
    let length = argument.len() + needs_slash as usize;
    if length > buffer.len() {
        return None;
    }

    let mut offset = 0;
    if needs_slash {
        buffer[0] = b'/';
        offset = 1;
    }
    buffer[offset..offset + argument.len()].copy_from_slice(argument);

    core::str::from_utf8(&buffer[..length]).ok()
}

fn parse_u64(input: &[u8]) -> Option<u64> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for &byte in input {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u64)?;
    }
    Some(value)
}

fn print_padded(value: u64, width: usize) {
    let mut divisor = 1;
    for _ in 1..width {
        divisor *= 10;
    }
    while divisor > 1 && value < divisor {
        k::print("0");
        divisor /= 10;
    }
    k::print_u64(value);
}

fn report_status(pid: u64, status: Option<u64>) {
    match status {
        Some(code) => {
            if pid == 0 {
                k::print("child exited with status ");
            } else {
                k::print("pid ");
                k::print_u64(pid);
                k::print(" exited with status ");
            }
            k::print_u64(code);
            k::print("\n");
        }
        None => k::print("wait failed\n"),
    }
}

/// Execute one command line. `exit` halts the machine and `exec` replaces the
/// calling process, exactly as in the CLI; a GUI caller that wants different
/// behaviour for those should intercept them before calling this.
pub fn exec(input: &[u8]) {
    let input = trim(input);
    if input.is_empty() {
        return;
    }

    let (command, rest) = split_word(input);
    let argument = trim(rest);

    let mut path_buffer = [0u8; 96];
    let mut file_buffer = [0u8; 512];
    let mut listing = [0u8; 256];

    if command == &b"help"[..] {
        k::print("  ls             list files with sizes\n");
        k::print("  cat <file>     print a file (RamFS or /disk/...)\n");
        k::print("  write <path> <text>  create/truncate a /disk file\n");
        k::print("  rm <file>      remove a /disk file\n");
        k::print("  mkdir <dir>    create a /disk directory\n");
        k::print("  rmdir <dir>    remove an empty /disk directory\n");
        k::print("  mv <old> <new> rename a /disk file\n");
        k::print("  stat <file>    show kind, size and blocks\n");
        k::print("  net [timeout]  bring up networking (DHCP) and show status\n");
        k::print("  dns <host>     resolve a hostname\n");
        k::print("  fetch <host> [path]  HTTP GET over TCP\n");
        k::print("  date           show UTC wall-clock time\n");
        k::print("  df             show disk geometry, files and free blocks\n");
        k::print("  time           show milliseconds since boot\n");
        k::print("  sleep <ms>     sleep while other processes keep running\n");
        k::print("  spawn <file>   create a process from an ELF or PE\n");
        k::print("  run <file>     spawn and wait for a process\n");
        k::print("  wait <pid>     wait for a child (0 = any)\n");
        k::print("  mem            report free physical frames\n");
        k::print("  ps             list processes\n");
        k::print("  kill <pid>     kill a process\n");
        k::print("  uname          show system identification\n");
        k::print("  uptime         show seconds since boot\n");
        k::print("  kpkg <sub>     package manager (update/list/install/run)\n");
        k::print("  kplugin <sub>  plugins (list/run/enable/disable/uninstall/import-alpine/import-py/catalog)\n");
        k::print("  dvm <file.dex> run a DEX file on the mini-Dalvik VM\n");
        k::print("  apk <file.apk> inspect an APK\n");
        k::print("  gfxbench       run the display-path benchmark\n");
        k::print("  exec <file>    replace the shell with an ELF or PE\n");
        k::print("  exit           halt the machine\n");
    } else if command == &b"ls"[..] {
        let (count, listing) = if argument.is_empty() {
            (k::list(&mut listing), &listing[..])
        } else {
            let Some(path) = with_slash(argument, &mut path_buffer) else {
                k::print("usage: ls [dir]\n");
                return;
            };
            match k::listdir(path, &mut listing) {
                Some(count) => (count, &listing[..]),
                None => {
                    k::print("ls: not a directory (disk paths only)\n");
                    return;
                }
            }
        };
        for name in listing[..count].split(|&byte| byte == 0) {
            if !name.is_empty() {
                k::print_bytes(name);
                k::print("\n");
            }
        }
    } else if command == &b"cat"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: cat <file>\n");
            return;
        };

        let Some(handle) = k::open(path) else {
            k::print("cat: file not found\n");
            return;
        };

        let mut offset = 0;
        loop {
            let count = k::read_file(handle, offset, &mut file_buffer);
            if count == 0 {
                break;
            }
            k::print_bytes(&file_buffer[..count]);
            offset += count;
        }
        k::close(handle);
    } else if command == &b"write"[..] {
        let (path_word, text) = split_word(argument);
        let Some(path) = with_slash(path_word, &mut path_buffer) else {
            k::print("usage: write /disk/<file> <text>\n");
            return;
        };
        let Some(handle) = k::create(path) else {
            k::print("write: no disk or bad path\n");
            return;
        };
        let count = k::write_file(handle, 0, text);
        if count != text.len() {
            k::print("write: short write\n");
        }
        if k::write_file(handle, text.len(), b"\n") != 1 {
            k::print("write: short write\n");
        }
        k::close(handle);
    } else if command == &b"df"[..] {
        match k::diskinfo() {
            Some(info) => {
                let total = info >> 32;
                let files = info & 0xffff_ffff;
                k::print("disk sectors: ");
                k::print_u64(total);
                k::print(", files: ");
                k::print_u64(files);
                k::print("\n");
            }
            None => k::print("df: no disk mounted\n"),
        }
        if let Some(free) = k::free_blocks() {
            k::print("free data blocks: ");
            k::print_u64(free);
            k::print("\n");
        }
    } else if command == &b"rm"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: rm <file>\n");
            return;
        };
        if k::unlink(path) {
            k::print("removed ");
            k::print(path);
            k::print("\n");
        } else {
            k::print("rm: failed (disk files only)\n");
        }
    } else if command == &b"mkdir"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: mkdir <dir>\n");
            return;
        };
        if k::mkdir(path) {
            k::print("created ");
            k::print(path);
            k::print("\n");
        } else {
            k::print("mkdir: failed (disk path, name must be free)\n");
        }
    } else if command == &b"rmdir"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: rmdir <dir>\n");
            return;
        };
        if k::rmdir(path) {
            k::print("removed ");
            k::print(path);
            k::print("\n");
        } else {
            k::print("rmdir: failed (empty disk directories only)\n");
        }
    } else if command == &b"mv"[..] {
        let (old_word, new_word) = split_word(argument);
        let mut new_buffer = [0u8; 64];
        let (Some(old), Some(new)) = (
            with_slash(old_word, &mut path_buffer),
            with_slash(new_word, &mut new_buffer),
        ) else {
            k::print("usage: mv <old> <new>\n");
            return;
        };
        if k::rename(old, new) {
            k::print("renamed ");
            k::print(old);
            k::print(" -> ");
            k::print(new);
            k::print("\n");
        } else {
            k::print("mv: failed (disk files only, new name must be free)\n");
        }
    } else if command == &b"time"[..] {
        k::print("uptime ms: ");
        k::print_u64(k::uptime_ms());
        k::print("\n");
    } else if command == &b"uptime"[..] {
        k::print("up ");
        k::print_u64(k::uptime_ms() / 1000);
        k::print(" seconds\n");
    } else if command == &b"uname"[..] {
        k::print("KimiOS 0.1 aarch64 bare metal\n");
    } else if command == &b"sleep"[..] {
        let Some(ms) = parse_u64(argument) else {
            k::print("usage: sleep <ms>\n");
            return;
        };
        let before = k::uptime_ms();
        k::sleep_ms(ms);
        let after = k::uptime_ms();
        k::print("slept ");
        k::print_u64(after.saturating_sub(before));
        k::print(" ms\n");
    } else if command == &b"spawn"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: spawn <file>\n");
            return;
        };

        match k::spawn(path) {
            Some(pid) => {
                k::print("spawned pid ");
                k::print_u64(pid);
                k::print("\n");
            }
            None => k::print("spawn failed\n"),
        }
    } else if command == &b"mem"[..] {
        k::print("free frames: ");
        k::print_u64(k::free_frames());
        k::print("\n");
    } else if command == &b"wait"[..] {
        let Some(pid) = parse_u64(argument) else {
            k::print("usage: wait <pid>\n");
            return;
        };
        report_status(pid, k::wait(pid));
    } else if command == &b"run"[..] {
        let (file_word, rest) = split_word(argument);
        let Some(path) = with_slash(file_word, &mut path_buffer) else {
            k::print("usage: run <file> [args...]\n");
            return;
        };
        let mut args: [&str; 16] = [""; 16];
        let mut argc = 0usize;
        let mut rem = trim(rest);
        while !rem.is_empty() && argc < 16 {
            let (word, next) = split_word(rem);
            if let Ok(s) = core::str::from_utf8(word) {
                if !s.is_empty() {
                    args[argc] = s;
                    argc += 1;
                }
            }
            rem = trim(next);
        }
        let spawned = if argc > 0 {
            k::spawn_args(path, &args[..argc])
        } else {
            k::spawn(path)
        };
        match spawned {
            Some(pid) => {
                k::print("spawned pid ");
                k::print_u64(pid);
                k::print("\n");
                report_status(pid, k::wait(pid));
            }
            None => k::print("spawn failed\n"),
        }
    } else if command == &b"exec"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: exec <file>\n");
            return;
        };

        k::print("exec: loading ");
        k::print(path);
        k::print("\n");
        k::exec(path);
        k::print("exec failed\n");
    } else if command == &b"stat"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: stat <file>\n");
            return;
        };
        match k::stat(path) {
            Some((kind, size)) => {
                k::print("kind: ");
                k::print(match kind {
                    0 => "ramfs",
                    2 => "directory",
                    _ => "disk file",
                });
                k::print(", size: ");
                k::print_u64(size);
                k::print(" bytes\n");
            }
            None => k::print("stat: not found\n"),
        }
    } else if command == &b"apk"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: apk <file.apk>\n");
            return;
        };
        let mut info = k::apk::ApkInfo::new();
        if !k::apk::inspect_apk(path, &mut info) {
            k::print("apk: not a readable APK\n");
            return;
        }
        k::print("package:    ");
        k::print_bytes(&info.package[..info.package_len]);
        k::print("\nversion:    ");
        k::print_bytes(&info.version_name[..info.version_name_len]);
        k::print(" (code ");
        k::print_u64(info.version_code as u64);
        k::print(")\nlabel:      ");
        k::print_bytes(&info.label[..info.label_len]);
        k::print("\nsdk:        min ");
        k::print_u64(info.min_sdk as u64);
        k::print(", target ");
        k::print_u64(info.target_sdk as u64);
        k::print("\nfiles:      ");
        k::print_u64(info.file_count as u64);
        k::print("\npermissions (");
        k::print_u64(info.permission_count as u64);
        k::print("):\n");
        for i in 0..info.permission_count {
            k::print("  ");
            k::print_bytes(&info.permissions[i][..info.permission_lens[i]]);
            k::print("\n");
        }
        k::print("activities (");
        k::print_u64(info.activity_count as u64);
        k::print("):\n");
        for i in 0..info.activity_count {
            k::print("  ");
            k::print_bytes(&info.activities[i][..info.activity_lens[i]]);
            k::print("\n");
        }
        if info.has_dex {
            k::print("dex:        v");
            k::print_bytes(&info.dex_version[..3]);
            k::print(", classes ");
            k::print_u64(info.dex_classes as u64);
            k::print(", methods ");
            k::print_u64(info.dex_methods as u64);
            k::print(", size ");
            k::print_u64(info.dex_size as u64);
            k::print("\n");
        }
        k::print("(inspection only — running APKs needs the Android runtime)\n");
    } else if command == &b"dvm"[..] {
        let Some(path) = with_slash(argument, &mut path_buffer) else {
            k::print("usage: dvm <file.dex>\n");
            return;
        };
        let ok = k::dvm_run_file(path);
        let mut out = [0u8; 2048];
        let n = k::dvm::take_output(&mut out);
        if n > 0 {
            k::print_bytes(&out[..n]);
        }
        if !ok {
            let e = k::dvm::last_error();
            k::print("dvm: ");
            k::print(if e.is_empty() { "cannot read DEX file" } else { e });
            k::print("\n");
        }
    } else if command == &b"gfxbench"[..] {
        k::gfx_bench();
    } else if command == &b"kpkg"[..] {
        let (sub, rest) = split_word(argument);
        let rest = trim(rest);
        match sub {
            b"update" => k::kpkg::update(core::str::from_utf8(rest).unwrap_or("")),
            b"list" => k::kpkg::list(),
            b"install" => match core::str::from_utf8(rest) {
                Ok(name) if !name.is_empty() => k::kpkg::install(name),
                _ => k::print("usage: kpkg install <name>\n"),
            },
            b"run" => {
                let (name_word, arest) = split_word(rest);
                match core::str::from_utf8(name_word) {
                    Ok(name) if !name.is_empty() => {
                        let mut args: [&str; 16] = [""; 16];
                        let mut argc = 0usize;
                        let mut rem = trim(arest);
                        while !rem.is_empty() && argc < 16 {
                            let (w, nx) = split_word(rem);
                            if let Ok(s) = core::str::from_utf8(w) {
                                if !s.is_empty() {
                                    args[argc] = s;
                                    argc += 1;
                                }
                            }
                            rem = trim(nx);
                        }
                        k::kpkg::run(name, &args[..argc]);
                    }
                    _ => k::print("usage: kpkg run <name> [args]\n"),
                }
            }
            _ => k::kpkg::help(),
        }
    } else if command == &b"apkg"[..] {
        let (sub, rest) = split_word(argument);
        let rest = trim(rest);
        match sub {
            b"update" => {
                let url = core::str::from_utf8(rest).unwrap_or("");
                k::print("apkg: fetching Alpine APKINDEX (main + community)...\n");
                if k::alpine::update(url) {
                    k::print("apkg: index ready\n");
                } else {
                    k::print("apkg: update failed (HTTP repo reachable?)\n");
                }
            }
            b"list" => k::alpine::list(),
            b"install" => match core::str::from_utf8(rest) {
                Ok(name) if !name.is_empty() => { k::alpine::install(name); }
                _ => k::print("usage: apkg install <name>\n"),
            },
            b"info" => match core::str::from_utf8(rest) {
                Ok(name) if !name.is_empty() => k::alpine::info(name),
                _ => k::print("usage: apkg info <name>\n"),
            },
            _ => {
                k::print("apkg — Alpine Linux apk compatibility\n");
                k::print("  apkg update [root-url]  fetch APKINDEX (default Alpine v3.20)\n");
                k::print("  apkg list               list indexed packages\n");
                k::print("  apkg install <name>     download + extract pkg + deps to /disk/alpine/root\n");
                k::print("  apkg info <name>        show version/size/deps\n");
                k::print("e.g. apkg install musl / python3 / nodejs\n");
            }
        }
    } else if command == &b"kplugin"[..] {
        let (sub, rest) = split_word(argument);
        let rest = trim(rest);
        match sub {
            b"list" => {
                const EMPTY: k::kplugin::PluginInfo = k::kplugin::PluginInfo {
                    id: [0; 48], id_len: 0, name: [0; 48], name_len: 0,
                    version: [0; 16], version_len: 0, ptype: 0, runtime: 0, enabled: false,
                };
                let mut infos = [EMPTY; 32];
                let n = k::kplugin::list(&mut infos);
                if n == 0 {
                    k::print("kplugin: none installed\n");
                }
                for i in 0..n {
                    let p = &infos[i];
                    k::print("  ");
                    k::print(p.id_str());
                    k::print("  ");
                    k::print(p.name_str());
                    k::print("  ");
                    k::print(p.type_str());
                    k::print(if p.enabled { "  on\n" } else { "  off\n" });
                }
            }
            b"run" => match core::str::from_utf8(rest) {
                Ok(id) if !id.is_empty() => match k::kplugin::run(id) {
                    Some(code) => {
                        k::print("kplugin: exit ");
                        k::print_u64(code);
                        k::print("\n");
                    }
                    None => k::print("kplugin: run failed (not installed?)\n"),
                },
                _ => k::print("usage: kplugin run <id>\n"),
            },
            b"enable" | b"disable" => {
                let on = sub == b"enable";
                match core::str::from_utf8(rest) {
                    Ok(id) if !id.is_empty() => {
                        if k::kplugin::set_enabled(id, on) {
                            k::print(if on { "kplugin: enabled\n" } else { "kplugin: disabled\n" });
                        } else {
                            k::print("kplugin: toggle failed\n");
                        }
                    }
                    _ => k::print("usage: kplugin enable|disable <id>\n"),
                }
            }
            b"uninstall" => match core::str::from_utf8(rest) {
                Ok(id) if !id.is_empty() => {
                    if k::kplugin::uninstall(id) {
                        k::print("kplugin: uninstalled\n");
                    } else {
                        k::print("kplugin: uninstall failed\n");
                    }
                }
                _ => k::print("usage: kplugin uninstall <id>\n"),
            },
            b"import-alpine" => match core::str::from_utf8(rest) {
                Ok(pkg) if !pkg.is_empty() => {
                    k::print("kplugin: importing Alpine package ");
                    k::print(pkg);
                    k::print(" (download + wrap)...\n");
                    if k::kimport::import_alpine(pkg) {
                        k::print("kplugin: imported & enabled (id alp.");
                        k::print(pkg);
                        k::print(")\n");
                    } else {
                        k::print("kplugin: import failed (apkg update first? network?)\n");
                    }
                }
                _ => k::print("usage: kplugin import-alpine <pkg>\n"),
            },
            b"import-py" => {
                let (url_word, rest2) = split_word(rest);
                let (id_word, name_rest) = split_word(rest2);
                let url = core::str::from_utf8(url_word).unwrap_or("");
                let id = core::str::from_utf8(id_word).unwrap_or("");
                let name = core::str::from_utf8(trim(name_rest)).unwrap_or("");
                if url.is_empty() || id.is_empty() {
                    k::print("usage: kplugin import-py <url> <id> <name>\n");
                } else if k::kimport::import_python_url(url, id, name) {
                    k::print("kplugin: python plugin imported & enabled\n");
                } else {
                    k::print("kplugin: python import failed (fetch/manifest)\n");
                }
            }
            b"catalog" => {
                let (filter_word, page_word) = split_word(rest);
                let filter = core::str::from_utf8(filter_word).unwrap_or("");
                let page = parse_u64(page_word).unwrap_or(0) as usize;
                catalog_cmd(filter, page);
            }
            _ => {
                k::print("kplugin — native guest plugins\n");
                k::print("  kplugin list                    installed plugins\n");
                k::print("  kplugin run <id>                run an app plugin\n");
                k::print("  kplugin enable|disable <id>     toggle enable state\n");
                k::print("  kplugin uninstall <id>          remove a plugin\n");
                k::print("  kplugin import-alpine <pkg>     install Alpine pkg + wrap as plugin\n");
                k::print("  kplugin import-py <url> <id> <name>  wrap a raw .py URL\n");
                k::print("  kplugin catalog [filter] [page] browse installable Alpine tools\n");
            }
        }
    } else if command == &b"ui"[..] || command == &b"uiread"[..] {
        let mut buf = [0u8; 8192];
        match k::ui_read(&mut buf) {
            Some(n) if n > 0 => {
                k::print_bytes(&buf[..n]);
                if buf[n - 1] != b'\n' {
                    k::print("\n");
                }
            }
            _ => k::print("ui: no snapshot. Enable Android Settings > Accessibility > KimiOS,\nthen open the app you want to read and run `ui` again.\n"),
        }
    } else if command == &b"net"[..] {
        let timeout = parse_u64(argument).unwrap_or(5000);
        k::net_status(timeout);
    } else if command == &b"dns"[..] {
        let Ok(host) = core::str::from_utf8(argument) else {
            k::print("usage: dns <hostname>\n");
            return;
        };
        k::net_status(5000);
        match k::net_resolve(host) {
            Some(ip) => {
                k::print(host);
                k::print(" -> ");
                let b = ip.to_be_bytes();
                k::print_u64(b[0] as u64);
                k::print(".");
                k::print_u64(b[1] as u64);
                k::print(".");
                k::print_u64(b[2] as u64);
                k::print(".");
                k::print_u64(b[3] as u64);
                k::print("\n");
            }
            None => k::print("dns: resolution failed\n"),
        }
    } else if command == &b"fetch"[..] {
        let (host_word, path_word) = split_word(argument);
        let Ok(host) = core::str::from_utf8(host_word) else {
            k::print("usage: fetch <host> [path]\n");
            return;
        };
        let path: &[u8] = if path_word.is_empty() { b"/" } else { path_word };
        k::net_status(5000);
        let Some(ip) = k::net_resolve(host) else {
            k::print("fetch: dns failed\n");
            return;
        };
        let Some(slot) = k::net_tcp_connect(ip, 80, 5000) else {
            k::print("fetch: connect failed\n");
            return;
        };
        let mut request = [0u8; 320];
        let mut used = 0usize;
        for piece in [
            &b"GET "[..],
            path,
            b" HTTP/1.0\r\nHost: ",
            host_word,
            b"\r\nConnection: close\r\n\r\n",
        ] {
            if used + piece.len() > request.len() {
                break;
            }
            request[used..used + piece.len()].copy_from_slice(piece);
            used += piece.len();
        }
        k::net_tcp_send(slot, &request[..used]);
        let mut buffer = [0u8; 512];
        let mut total = 0usize;
        loop {
            let count = k::net_tcp_recv(slot, &mut buffer, 3000);
            if count == 0 {
                break;
            }
            k::print_bytes(&buffer[..count]);
            total += count;
            if total > 4096 {
                k::print("\n[truncated]\n");
                break;
            }
        }
        k::net_tcp_close(slot);
        k::print("\n");
    } else if command == &b"date"[..] {
        match k::rtc() {
            Some(clock) => {
                print_padded(clock[1], 4);
                k::print("-");
                print_padded(clock[2], 2);
                k::print("-");
                print_padded(clock[3], 2);
                k::print(" ");
                print_padded(clock[4], 2);
                k::print(":");
                print_padded(clock[5], 2);
                k::print(":");
                print_padded(clock[6], 2);
                k::print(" UTC (");
                k::print_u64(clock[0]);
                k::print(")\n");
            }
            None => k::print("date: no RTC\n"),
        }
    } else if command == &b"ps"[..] {
        let mut rows = [0u64; 64];
        let count = k::ps(&mut rows);
        k::print("PID PPID STATE EXIT\n");
        for index in 0..count {
            let base = index * 4;
            k::print_u64(rows[base]);
            k::print(" ");
            k::print_u64(rows[base + 1]);
            k::print(" ");
            k::print(match rows[base + 2] {
                0 => "free ",
                1 => "ready",
                2 => "run  ",
                3 => "block",
                _ => "zombi",
            });
            k::print(" ");
            k::print_u64(rows[base + 3]);
            k::print("\n");
        }
    } else if command == &b"kill"[..] {
        let Some(pid) = parse_u64(argument) else {
            k::print("usage: kill <pid>\n");
            return;
        };
        if k::kill(pid) {
            k::print("killed pid ");
            k::print_u64(pid);
            k::print("\n");
        } else {
            k::print("kill: no such live process\n");
        }
    } else if command == &b"exit"[..] {
        k::print("bye\n");
        k::exit(0);
    } else if command == &b"node"[..] || command == &b"nodejs"[..] {
        let arg = core::str::from_utf8(trim(argument)).unwrap_or("");
        linux_guest("usr/bin/node", arg);
    } else if command == &b"npm"[..] || command == &b"npx"[..] {
        let arg = core::str::from_utf8(trim(argument)).unwrap_or("");
        linux_guest(core::str::from_utf8(command).unwrap_or("npm"), arg);
    } else if command == &b"python"[..] || command == &b"python3"[..] {
        let arg = core::str::from_utf8(trim(argument)).unwrap_or("");
        linux_guest("usr/bin/python3", arg);
    } else if command == &b"alpine"[..] {
        let (bin_word, rest) = split_word(argument);
        let bin = core::str::from_utf8(bin_word).unwrap_or("");
        if bin.is_empty() {
            k::print("usage: alpine <bin> [arg]   e.g. alpine usr/bin/node 'console.log(1)'\n");
            return;
        }
        let arg = core::str::from_utf8(trim(rest)).unwrap_or("");
        linux_guest(bin, arg);
    } else {
        let mut bin_buffer = [0u8; 160];
        if !command.contains(&b'/') {
            if let Some(bin) = resolve_sysroot_bin(command, &mut bin_buffer) {
                linux_guest(bin, core::str::from_utf8(argument).unwrap_or(""));
                return;
            }
            if disk_bin_app(command, argument) {
                return;
            }
        }
        k::print("unknown command: ");
        k::print_bytes(command);
        k::print(" (try `apkg install ");
        k::print_bytes(command);
        k::print("` first)\n");
    }
}

/// Resolve a bare command name against the Alpine sysroot PATH, returning the
/// sysroot-relative binary path (e.g. "usr/bin/curl") for `linux_run`.
fn resolve_sysroot_bin<'a>(command: &[u8], buffer: &'a mut [u8; 160]) -> Option<&'a str> {
    let mut found = None;
    for dir in [&b"usr/bin/"[..], b"bin/", b"usr/sbin/", b"sbin/"] {
        let length = dir.len() + command.len();
        if length > buffer.len() {
            continue;
        }
        buffer[..dir.len()].copy_from_slice(dir);
        buffer[dir.len()..length].copy_from_slice(command);
        let Ok(rel) = core::str::from_utf8(&buffer[..length]) else {
            continue;
        };
        let mut full = [0u8; 180];
        let prefix = b"/disk/alpine/root/";
        let total = prefix.len() + length;
        if total > full.len() {
            continue;
        }
        full[..prefix.len()].copy_from_slice(prefix);
        full[prefix.len()..total].copy_from_slice(rel.as_bytes());
        let Ok(path) = core::str::from_utf8(&full[..total]) else {
            continue;
        };
        if let Some((kind, _)) = k::stat(path) {
            if kind != 2 {
                found = Some(length);
                break;
            }
        }
    }
    core::str::from_utf8(&buffer[..found?]).ok()
}

/// kpkg-installed static apps in /disk/bin: spawn on bare metal; the Android
/// libOS host has no spawn, so point the user at `kpkg run`.
fn disk_bin_app(command: &[u8], argument: &[u8]) -> bool {
    let mut path = [0u8; 96];
    let prefix = b"/disk/bin/";
    let length = prefix.len() + command.len();
    if length > path.len() {
        return false;
    }
    path[..prefix.len()].copy_from_slice(prefix);
    path[prefix.len()..length].copy_from_slice(command);
    let Ok(path_str) = core::str::from_utf8(&path[..length]) else {
        return false;
    };
    if k::stat(path_str).is_none() {
        return false;
    }
    let mut args: [&str; 16] = [""; 16];
    let mut argc = 0usize;
    let mut rem = argument;
    while !rem.is_empty() && argc < 16 {
        let (word, next) = split_word(rem);
        if let Ok(s) = core::str::from_utf8(word) {
            if !s.is_empty() {
                args[argc] = s;
                argc += 1;
            }
        }
        rem = trim(next);
    }
    let spawned = if argc > 0 {
        k::spawn_args(path_str, &args[..argc])
    } else {
        k::spawn(path_str)
    };
    match spawned {
        Some(pid) => {
            report_status(pid, k::wait(pid));
            true
        }
        None => {
            k::print_bytes(command);
            k::print(": static app — run it with `kpkg run ");
            k::print_bytes(command);
            k::print("` (no spawn on the Android libOS host)\n");
            true
        }
    }
}

fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| {
        w.iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
    })
}

/// Browse the Alpine command catalog (first 256 index entries, no downloads),
/// filtered by a case-insensitive substring of name/description, 20 per page.
fn catalog_cmd(filter: &str, page: usize) {
    const CAP: usize = 256;
    const PER_PAGE: usize = 20;
    const EMPTY: k::alpine::CmdPkg = k::alpine::CmdPkg {
        name: [0; 32], name_len: 0, desc: [0; 64], desc_len: 0, version: [0; 16], version_len: 0,
    };
    static mut BUF: [k::alpine::CmdPkg; CAP] = [EMPTY; CAP];
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
    let n = k::alpine::list_command_packages(buf);
    if n == 0 {
        k::print("kplugin: catalog empty — run `apkg update` first\n");
        return;
    }
    let fb = filter.as_bytes();
    let mut matched = 0usize;
    let mut shown = 0usize;
    let start = page * PER_PAGE;
    for i in 0..n {
        let p = &buf[i];
        if !contains_ci(&p.name[..p.name_len], fb) && !contains_ci(&p.desc[..p.desc_len], fb) {
            continue;
        }
        if matched >= start && shown < PER_PAGE {
            k::print("  ");
            k::print_bytes(&p.name[..p.name_len]);
            k::print("  ");
            k::print_bytes(&p.version[..p.version_len]);
            k::print("  ");
            k::print_bytes(&p.desc[..p.desc_len]);
            k::print("\n");
            shown += 1;
        }
        matched += 1;
    }
    let pages = (matched + PER_PAGE - 1) / PER_PAGE;
    k::print("kplugin: ");
    k::print_u64(matched as u64);
    k::print(" matches, page ");
    k::print_u64((page + 1) as u64);
    k::print("/");
    k::print_u64(pages.max(1) as u64);
    k::print(" (buffer holds first ");
    k::print_u64(n as u64);
    k::print(" catalog entries; install with `kplugin import-alpine <name>`)\n");
}

/// Run an Alpine sysroot binary natively (Android libOS) and print its output.
fn linux_guest(bin: &str, arg: &str) {
    let mut out = [0u8; 4096];
    match k::linux_run(bin, arg, &mut out) {
        Some((code, n)) => {
            k::print_bytes(&out[..n]);
            if n > 0 && out[n - 1] != b'\n' {
                k::print("\n");
            }
            k::print("[exit ");
            k::print_u64(code);
            k::print("]\n");
        }
        None => k::print("linux guests need the Android libOS host (apkg install first)\n"),
    }
}
