use crate::draw::Screen;
use crate::AppKind;
use libkimi as k;
use crate::theme::{BLACK, SURFACE, SURFACE_HI, SURFACE_MAX, OUTLINE, TEXT, MUTED, RED, RED_DIM, GREEN};


const TERM_WIDTH: usize = 50;
const TERM_LINES: usize = 40;

pub struct Music {
    track: usize,
    count: usize,
    names: [[u8; 48]; 16],
    name_len: [u8; 16],
    playing: bool,
    pos_ms: u64,
    dur_ms: u64,
    scroll: usize,
    status: [u8; 48],
    status_len: usize,
}

pub struct Notes {
    text: [u8; 1024],
    len: usize,
    editing: Option<usize>,
    count: usize,
    titles: [[u8; 32]; 16],
    title_len: [u8; 16],
    paths: [[u8; 48]; 16],
    path_len: [u8; 16],
    scroll: usize,
    status: [u8; 48],
    status_len: usize,
}

pub struct Maps {
    pin: Option<(i64, i64)>,
    has_fix: bool,
    lat: i64,
    lon: i64,
    accuracy: u64,
    status: [u8; 64],
    status_len: usize,
}

pub struct Photos {
    selected: Option<usize>,
}

pub struct Files {
    dir: [u8; 128],
    dir_len: usize,
    count: usize,
    names: [[u8; 48]; 64],
    name_len: [u8; 64],
    paths: [[u8; 128]; 64],
    path_len: [u8; 64],
    sizes: [u64; 64],
    is_dir: [bool; 64],
    scroll: usize,
    preview: Option<usize>,
    body: [u8; 768],
    body_len: usize,
    binary: bool,
    status: [u8; 48],
    status_len: usize,
    action: Option<usize>,
    confirm_delete: bool,
    prompt: u8,
    prompt_buf: [u8; 48],
    prompt_len: usize,
    editor: Option<usize>,
    edit: [u8; 2048],
    edit_len: usize,
    clip_path: [u8; 128],
    clip_len: usize,
    clip_name: [u8; 48],
    clip_name_len: usize,
    clip_cut: bool,
    props: Option<usize>,
    run_show: bool,
    run_msg: [u8; 64],
    run_len: usize,
}

#[derive(Clone, Copy)]
pub struct Proc {
    pid: u64,
    ppid: u64,
    ram_kb: u64,
    cpu_x10: u64,
    state: u64,
    name: [u8; 24],
    name_len: usize,
}

const EMPTY_PROC: Proc = Proc {
    pid: 0, ppid: 0, ram_kb: 0, cpu_x10: 0, state: 0,
    name: [0; 24], name_len: 0,
};

pub struct Monitor {
    info: [u64; 10],
    has_info: bool,
    procs: [Proc; 40],
    proc_count: usize,
    scroll: usize,
    selected: Option<u64>,
    kill_msg: [u8; 48],
    kill_len: usize,
    fps_x10: u64,
    cpu_hist: [u16; 48],
    hist_len: usize,
    active: [[u8; 16]; 8],
    active_len: [u8; 8],
    active_count: usize,
    last_refresh: u64,
}

impl Monitor {
    fn new() -> Self {
        Self {
            info: [0; 10],
            has_info: false,
            procs: [EMPTY_PROC; 40],
            proc_count: 0,
            scroll: 0,
            selected: None,
            kill_msg: [0; 48],
            kill_len: 0,
            fps_x10: 0,
            cpu_hist: [0; 48],
            hist_len: 0,
            active: [[0; 16]; 8],
            active_len: [0; 8],
            active_count: 0,
            last_refresh: 0,
        }
    }

    fn refresh(&mut self) {
        self.last_refresh = k::uptime_ms();
        if let Some(info) = k::sysinfo() {
            self.info = info;
            self.has_info = true;
            let cpu = (info[0] / 10).min(100) as u16;
            if self.hist_len < self.cpu_hist.len() {
                self.cpu_hist[self.hist_len] = cpu;
                self.hist_len += 1;
            } else {
                for i in 1..self.cpu_hist.len() {
                    self.cpu_hist[i - 1] = self.cpu_hist[i];
                }
                self.cpu_hist[self.cpu_hist.len() - 1] = cpu;
            }
        }
        let mut raw = [0u64; 320];
        let n = k::proclist(&mut raw);
        self.proc_count = 0;
        for rec in raw[..n * 8].chunks_exact(8) {
            if self.proc_count >= self.procs.len() {
                break;
            }
            let mut name = [0u8; 24];
            for j in 0..3 {
                name[j * 8..j * 8 + 8].copy_from_slice(&rec[5 + j].to_le_bytes());
            }
            let name_len = name.iter().position(|&b| b == 0).unwrap_or(24);
            self.procs[self.proc_count] = Proc {
                pid: rec[0],
                ppid: rec[1],
                ram_kb: rec[2],
                cpu_x10: rec[3],
                state: rec[4],
                name,
                name_len,
            };
            self.proc_count += 1;
        }
        if self.scroll + 10 > self.proc_count {
            self.scroll = self.proc_count.saturating_sub(10);
        }
    }

    fn find(&self, pid: u64) -> Option<&Proc> {
        self.procs[..self.proc_count].iter().find(|p| p.pid == pid)
    }

    fn kill_selected(&mut self) {
        let Some(pid) = self.selected else { return };
        let msg = if k::kill(pid) { "Process terminated" } else { "Kill failed (host may restrict it)" };
        let b = msg.as_bytes();
        self.kill_len = b.len().min(48);
        self.kill_msg[..self.kill_len].copy_from_slice(&b[..self.kill_len]);
        self.refresh();
    }
}

pub fn monitor_sync(apps: &mut Apps, recents: &[AppKind], fps_x10: u64) {
    let m = &mut apps.monitor;
    m.fps_x10 = fps_x10;
    m.active_count = 0;
    for &app in recents {
        if m.active_count >= 8 {
            break;
        }
        let label = app.label().as_bytes();
        let len = label.len().min(16);
        let i = m.active_count;
        m.active[i][..len].copy_from_slice(&label[..len]);
        m.active_len[i] = len as u8;
        m.active_count += 1;
    }
}

#[derive(Clone, Copy)]
struct ForecastDay {
    week: [u8; 3],
    icon: u64,
    max: i64,
    min: i64,
}

const FEELS_NONE: i64 = i64::MIN;

pub struct Weather {
    fahrenheit: bool,
    has_fix: bool,
    has_data: bool,
    temp: i64,
    feels: i64,
    humidity: i64,
    wind: i64,
    code: u64,
    provider: u8,
    cond: [u8; 24],
    cond_len: usize,
    lat: i64,
    lon: i64,
    accuracy: u64,
    forecast: [ForecastDay; 9],
    forecast_count: usize,
    status: [u8; 64],
    status_len: usize,
}

impl Weather {
    fn new() -> Self {
        Self {
            fahrenheit: false,
            has_fix: false,
            has_data: false,
            temp: 0,
            feels: FEELS_NONE,
            humidity: -1,
            wind: -1,
            code: 0,
            provider: 0,
            cond: [0u8; 24],
            cond_len: 0,
            lat: 0,
            lon: 0,
            accuracy: 0,
            forecast: [ForecastDay { week: [b' '; 3], icon: 0, max: 0, min: 0 }; 9],
            forecast_count: 0,
            status: [0u8; 64],
            status_len: 0,
        }
    }

    fn set_cond(&mut self, text: &str) {
        self.cond_len = text.len().min(self.cond.len());
        self.cond[..self.cond_len].copy_from_slice(&text.as_bytes()[..self.cond_len]);
    }
}

pub struct Terminal {
    lines: [[u8; TERM_WIDTH]; TERM_LINES],
    line_len: [u8; TERM_LINES],
    count: usize,
    input: [u8; 64],
    input_len: usize,
}

const BROWSER_WIDTH: usize = 50;
const BROWSER_LINES: usize = 90;
const BROWSER_BODY: usize = 8000;

const WEB_X0: i64 = 20;
const WEB_Y0: i64 = 196;
const WEB_X1: i64 = 460;
const WEB_Y1: i64 = 700;

pub struct Browser {
    url: [u8; 128],
    url_len: usize,
    editing: bool,
    lines: [[u8; BROWSER_WIDTH]; BROWSER_LINES],
    line_len: [u8; BROWSER_LINES],
    line_count: usize,
    scroll: usize,
    status: [u8; 64],
    status_len: usize,
    title: [u8; 64],
    title_len: usize,
    web_ok: bool,
    web_started: bool,
    loaded: bool,
    last_seq: u64,
    last_force_ms: u64,
    fbw: i64,
    fbh: i64,
    stride: i64,
    rx: i64,
    ry: i64,
    rw: i64,
    rh: i64,
    page_url: [u8; 256],
    page_url_len: usize,
    page_title: [u8; 96],
    page_title_len: usize,
    can_back: bool,
    can_fwd: bool,
    touch_active: bool,
}

impl Browser {
    fn new() -> Self {
        let mut browser = Browser {
            url: [0u8; 128],
            url_len: 0,
            editing: true,
            lines: [[0u8; BROWSER_WIDTH]; BROWSER_LINES],
            line_len: [0u8; BROWSER_LINES],
            line_count: 0,
            scroll: 0,
            status: [0u8; 64],
            status_len: 0,
            title: [0u8; 64],
            title_len: 0,
            web_ok: false,
            web_started: false,
            loaded: false,
            last_seq: 0,
            last_force_ms: 0,
            fbw: 0,
            fbh: 0,
            stride: 0,
            rx: 0,
            ry: 0,
            rw: 0,
            rh: 0,
            page_url: [0u8; 256],
            page_url_len: 0,
            page_title: [0u8; 96],
            page_title_len: 0,
            can_back: false,
            can_fwd: false,
            touch_active: false,
        };
        let default = b"example.com";
        browser.url[..default.len()].copy_from_slice(default);
        browser.url_len = default.len();
        browser
    }

    fn set_status(&mut self, text: &str) {
        let length = text.len().min(self.status.len());
        self.status[..length].copy_from_slice(&text.as_bytes()[..length]);
        self.status_len = length;
    }

    fn init_web(&mut self) {
        if self.web_started {
            return;
        }
        self.web_started = true;
        let Some((w, h, stride, _)) = k::fb_info() else {
            return;
        };
        self.fbw = w as i64;
        self.fbh = h as i64;
        self.stride = (stride / 4) as i64;
        self.rx = WEB_X0 * self.fbw / 480;
        self.ry = WEB_Y0 * self.fbh / 800;
        self.rw = WEB_X1 * self.fbw / 480 - self.rx;
        self.rh = WEB_Y1 * self.fbh / 800 - self.ry;
        if self.rw <= 0 || self.rh <= 0 || self.stride < self.fbw {
            return;
        }
        self.web_ok = k::web_init(self.rw as u32, self.rh as u32);
    }

    fn open(&mut self) {
        self.init_web();
        if self.web_ok {
            k::web_resume();
            self.editing = false;
            if !self.loaded {
                self.loaded = true;
                if !k::web_load("https://example.com") {
                    self.web_ok = false;
                }
            }
        }
        if !self.web_ok {
            self.editing = self.line_count == 0;
        }
    }

    fn sync_meta(&mut self) {
        if !self.web_ok {
            return;
        }
        self.page_url_len = k::web_url(&mut self.page_url).min(self.page_url.len());
        self.page_title_len = k::web_title(&mut self.page_title).min(self.page_title.len());
        self.can_back = k::web_can_back();
        self.can_fwd = k::web_can_forward();
    }

    fn local_point(&self, point: (i64, i64)) -> (u32, u32) {
        let x = point.0 * self.fbw / 480 - self.rx;
        let y = point.1 * self.fbh / 800 - self.ry;
        (
            x.clamp(0, self.rw - 1) as u32,
            y.clamp(0, self.rh - 1) as u32,
        )
    }

    fn blit(&self, bottom_app: i64) -> i64 {
        if !self.web_ok {
            return 0;
        }
        let Some(src) = k::web_buf() else {
            return 0;
        };
        let Some(dst) = k::fb_back() else {
            return 0;
        };
        let mut h = self.rh;
        if bottom_app < WEB_Y1 {
            h = (bottom_app * self.fbh / 800 - self.ry).clamp(0, self.rh);
        }
        blit_rgba_to_xrgb(
            src,
            self.rw,
            h,
            dst as *mut u32,
            self.stride,
            self.rx,
            self.ry,
            self.fbw,
            self.fbh,
        )
    }
}

fn in_web_content(point: (i64, i64)) -> bool {
    point.0 >= WEB_X0 && point.0 < WEB_X1 && point.1 >= WEB_Y0 && point.1 < WEB_Y1
}

/// Bounds-checked RGBA→XRGB blit of a sw×sh source into the back buffer at
/// (dx,dy); clips to max_w×max_h. Returns the number of rows written.
pub fn blit_rgba_to_xrgb(
    src: *const u8,
    sw: i64,
    sh: i64,
    dst: *mut u32,
    stride: i64,
    dx: i64,
    dy: i64,
    max_w: i64,
    max_h: i64,
) -> i64 {
    if src.is_null() || dst.is_null() || sw <= 0 || sh <= 0 || dx < 0 || dy < 0 || stride <= 0 {
        return 0;
    }
    let w = sw.min(max_w - dx);
    let h = sh.min(max_h - dy);
    if w <= 0 || h <= 0 {
        return 0;
    }
    for y in 0..h {
        let s = unsafe { src.add((y * sw * 4) as usize) };
        let d = unsafe { dst.add(((dy + y) * stride + dx) as usize) };
        for x in 0..w {
            let (r, g, b) = unsafe {
                (
                    *s.add((x * 4) as usize) as u32,
                    *s.add((x * 4 + 1) as usize) as u32,
                    *s.add((x * 4 + 2) as usize) as u32,
                )
            };
            unsafe { *d.add(x as usize) = (r << 16) | (g << 8) | b };
        }
    }
    h
}

pub fn browser_pause(apps: &mut Apps) {
    let browser = &mut apps.browser;
    if browser.web_ok {
        if browser.touch_active {
            k::web_touch(0, 0, 3);
            browser.touch_active = false;
        }
        k::web_pause();
    }
}

pub fn browser_resume(apps: &mut Apps) {
    if apps.browser.web_ok {
        k::web_resume();
    }
}

pub fn browser_can_back(apps: &Apps) -> bool {
    apps.browser.web_ok && apps.browser.can_back
}

pub fn browser_web_back(apps: &mut Apps) {
    if apps.browser.web_ok && apps.browser.can_back {
        k::web_back();
        apps.browser.set_status("Back");
    }
}

/// Per-frame WebView pull while the Browser window is visible. Blits a new
/// stable frame into the content rect and presents it. Returns true on blit.
pub fn browser_frame(apps: &mut Apps, osk: bool) -> bool {
    let browser = &mut apps.browser;
    if !browser.web_ok {
        return false;
    }
    let now = k::uptime_ms();
    let force = now.saturating_sub(browser.last_force_ms) >= 100;
    if force {
        browser.last_force_ms = now;
    }
    let seq = k::web_render(force);
    if seq == 0 || seq & 1 != 0 || seq == browser.last_seq {
        return false;
    }
    browser.last_seq = seq;
    browser.sync_meta();
    let bottom = if osk { crate::keyboard::TOP } else { WEB_Y1 };
    let h = browser.blit(bottom);
    if h > 0 {
        k::fb_present(
            browser.rx as u64,
            browser.ry as u64,
            (browser.rx + browser.rw) as u64,
            (browser.ry + h) as u64,
        );
    }
    true
}

pub fn browser_touch_down(apps: &mut Apps, point: (i64, i64)) -> bool {
    let browser = &mut apps.browser;
    if !browser.web_ok || browser.touch_active || !in_web_content(point) {
        return false;
    }
    let (x, y) = browser.local_point(point);
    browser.touch_active = k::web_touch(x, y, 0);
    browser.touch_active
}

pub fn browser_touch_move(apps: &mut Apps, point: (i64, i64)) {
    let browser = &mut apps.browser;
    if !browser.touch_active {
        return;
    }
    let (x, y) = browser.local_point(point);
    if !in_web_content(point) {
        browser.touch_active = false;
        k::web_touch(x, y, 3);
        return;
    }
    k::web_touch(x, y, 2);
}

pub fn browser_touch_up(apps: &mut Apps, point: (i64, i64)) {
    let browser = &mut apps.browser;
    if !browser.touch_active {
        return;
    }
    browser.touch_active = false;
    let (x, y) = browser.local_point(point);
    k::web_touch(x, y, 1);
}

pub struct ApkApp {
    names: [[u8; 32]; 8],
    name_lens: [usize; 8],
    count: usize,
    selected: Option<usize>,
    info: k::apk::ApkInfo,
}

impl ApkApp {
    fn refresh(&mut self) {
        self.count = 0;
        self.selected = None;
        let mut buffer = [0u8; 512];
        if let Some(n) = k::listdir("/disk", &mut buffer) {
            for name in buffer[..n].split(|&b| b == 0) {
                if name.len() > 4 && &name[name.len() - 4..] == b".apk" && self.count < 8 {
                    let len = name.len().min(32);
                    self.names[self.count][..len].copy_from_slice(&name[..len]);
                    self.name_lens[self.count] = len;
                    self.count += 1;
                }
            }
        }
    }
}

pub struct DvmApp {
    names: [[u8; 32]; 8],
    name_lens: [usize; 8],
    count: usize,
    selected: Option<usize>,
    output: [u8; 1024],
    out_len: usize,
    ok: bool,
    err: [u8; 48],
    err_len: usize,
}

impl DvmApp {
    fn refresh(&mut self) {
        self.count = 0;
        self.selected = None;
        self.out_len = 0;
        self.ok = false;
        self.err_len = 0;
        let mut buffer = [0u8; 512];
        if let Some(n) = k::listdir("/disk", &mut buffer) {
            for name in buffer[..n].split(|&b| b == 0) {
                if name.len() > 4 && &name[name.len() - 4..] == b".dex" && self.count < 8 {
                    let len = name.len().min(32);
                    self.names[self.count][..len].copy_from_slice(&name[..len]);
                    self.name_lens[self.count] = len;
                    self.count += 1;
                }
            }
        }
    }
}

use libkimi::kplugin::{self, PluginInfo, TYPE_APP, TYPE_SERVICE, TYPE_UI};

const EMPTY_PLUGIN: PluginInfo = PluginInfo {
    id: [0; 48], id_len: 0, name: [0; 48], name_len: 0,
    version: [0; 16], version_len: 0, ptype: TYPE_APP, runtime: 0, enabled: false,
};

const PLUGIN_ROWS: usize = 8;

pub struct Plugins {
    items: [PluginInfo; 32],
    count: usize,
    scroll: usize,
    selected: Option<usize>,
    status: [u8; 64],
    status_len: usize,
    confirm_uninstall: bool,
    view: u8,
    import_paths: [[u8; 96]; 8],
    import_lens: [usize; 8],
    import_count: usize,
}

impl Plugins {
    fn new() -> Self {
        Self {
            items: [EMPTY_PLUGIN; 32],
            count: 0,
            scroll: 0,
            selected: None,
            status: [0; 64],
            status_len: 0,
            confirm_uninstall: false,
            view: 0,
            import_paths: [[0; 96]; 8],
            import_lens: [0; 8],
            import_count: 0,
        }
    }

    fn set_status(&mut self, text: &str) {
        self.status_len = text.len().min(64);
        self.status[..self.status_len].copy_from_slice(&text.as_bytes()[..self.status_len]);
    }

    fn refresh(&mut self) {
        self.count = kplugin::list(&mut self.items);
        self.confirm_uninstall = false;
        if self.scroll + PLUGIN_ROWS > self.count {
            self.scroll = self.count.saturating_sub(PLUGIN_ROWS);
        }
        if let Some(i) = self.selected {
            if i >= self.count {
                self.selected = None;
            }
        }
    }

    fn scan_imports(&mut self) {
        self.import_count = 0;
        k::kpkg::scan_plugin_imports(|path| {
            if self.import_count >= 8 {
                return;
            }
            let slot = self.import_count;
            let b = path.as_bytes();
            let n = b.len().min(96);
            self.import_paths[slot][..n].copy_from_slice(&b[..n]);
            self.import_lens[slot] = n;
            self.import_count += 1;
        });
    }

    fn install_import(&mut self, slot: usize) {
        if slot >= self.import_count {
            return;
        }
        let path = slice_str(&self.import_paths[slot][..self.import_lens[slot]]);
        if kplugin::install(path) {
            self.set_status("Plugin installed");
            self.view = 0;
            self.refresh();
        } else {
            self.set_status("Install failed (bad manifest?)");
        }
    }

    fn toggle_selected(&mut self) {
        let Some(i) = self.selected else { return };
        let info = self.items[i];
        let on = !info.enabled;
        if kplugin::set_enabled(info.id_str(), on) {
            self.items[i].enabled = on;
            self.set_status(if on { "Plugin enabled" } else { "Plugin disabled" });
        } else {
            self.set_status("Toggle failed");
        }
    }

    fn uninstall_selected(&mut self) {
        let Some(i) = self.selected else { return };
        if !self.confirm_uninstall {
            self.confirm_uninstall = true;
            return;
        }
        let id = self.items[i].id_str();
        if kplugin::uninstall(id) {
            self.set_status("Plugin uninstalled");
            self.selected = None;
            self.refresh();
        } else {
            self.set_status("Uninstall failed");
        }
    }

    fn primary_action(&mut self, screen: &Screen) {
        let Some(i) = self.selected else { return };
        let info = self.items[i];
        match info.ptype {
            TYPE_APP => {
                self.set_status("Running...");
                screen.reset_dirty();
                screen.text(170, 156, "Running...", 1, RED());
                if let Some(d) = screen.dirty_rect() {
                    k::fb_present(d[0] as u64, d[1] as u64, d[2] as u64, d[3] as u64);
                }
                match kplugin::run(info.id_str()) {
                    Some(code) => {
                        let mut line = [0u8; 64];
                        let mut used = 0;
                        let mut num = [0u8; 20];
                        push_str(&mut line, &mut used, b"Exited with code ");
                        push_str(&mut line, &mut used, unsigned(code, &mut num).as_bytes());
                        self.set_status(slice_str(&line[..used]));
                    }
                    None => self.set_status("Run unavailable on this host"),
                }
            }
            TYPE_UI => self.apply_ui_plugin(info),
            TYPE_SERVICE => {
                kplugin::start_services();
                self.set_status("Services started");
            }
            _ => {}
        }
    }

    fn apply_ui_plugin(&mut self, info: PluginInfo) {
        let id = info.id_str();
        if !kplugin::apply_ui(id) {
            self.set_status("No bundled theme found");
            return;
        }
        let mut path = [0u8; 160];
        let mut used = 0;
        push_str(&mut path, &mut used, b"/disk/plugins/");
        push_str(&mut path, &mut used, id.as_bytes());
        push_str(&mut path, &mut used, b"/plugin.json");
        let mut buf = [0u8; 2048];
        let n = read_disk_file(slice_str(&path[..used]), &mut buf);
        let data = &buf[..n];
        let mut applied = false;
        let mut asset = [0u8; 48];
        if let Some(an) = json_str(data, b"theme", &mut asset) {
            let mut tp = [0u8; 160];
            let mut tn = 0;
            push_str(&mut tp, &mut tn, b"/disk/plugins/");
            push_str(&mut tp, &mut tn, id.as_bytes());
            push_str(&mut tp, &mut tn, b"/");
            push_str(&mut tp, &mut tn, &asset[..an]);
            if crate::theme::load_theme_file(slice_str(&tp[..tn]), slice_str(&asset[..an])) {
                applied = true;
            }
        }
        if let Some(an) = json_str(data, b"font", &mut asset) {
            let mut fp = [0u8; 160];
            let mut fnn = 0;
            push_str(&mut fp, &mut fnn, b"/disk/plugins/");
            push_str(&mut fp, &mut fnn, id.as_bytes());
            push_str(&mut fp, &mut fnn, b"/");
            push_str(&mut fp, &mut fnn, &asset[..an]);
            if crate::font_store::load(slice_str(&fp[..fnn]), slice_str(&asset[..an])) {
                crate::theme::apply_font(4);
                applied = true;
            }
        }
        if applied {
            save_theme_cfg();
            self.set_status("UI plugin applied");
        } else {
            self.set_status("Theme/font parse failed");
        }
    }
}

pub struct PluginLauncher {
    items: [PluginInfo; 16],
    count: usize,
}

pub fn launcher_refresh(apps: &mut Apps) {
    let mut all = [EMPTY_PLUGIN; 32];
    let n = kplugin::list(&mut all);
    let launcher = &mut apps.plugin_home;
    launcher.count = 0;
    for i in 0..n {
        if all[i].ptype == TYPE_APP && all[i].enabled && launcher.count < launcher.items.len() {
            launcher.items[launcher.count] = all[i];
            launcher.count += 1;
        }
    }
}

pub fn launcher_count(apps: &Apps) -> usize {
    apps.plugin_home.count
}

pub fn launcher_name(apps: &Apps, index: usize) -> &str {
    apps.plugin_home.items[index].name_str()
}

pub fn launcher_run(apps: &mut Apps, index: usize) -> Option<u64> {
    if index >= apps.plugin_home.count {
        return None;
    }
    kplugin::run(apps.plugin_home.items[index].id_str())
}

pub struct Apps {
    pub music: Music,
    pub notes: Notes,
    pub maps: Maps,
    pub photos: Photos,
    pub files: Files,
    pub weather: Weather,
    pub terminal: Terminal,
    pub browser: Browser,
    pub apk: ApkApp,
    pub dvm: DvmApp,
    pub market: Market,
    pub monitor: Monitor,
    pub plugins: Plugins,
    pub plugin_home: PluginLauncher,
}

fn slice_str(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("")
}

// theme::save now persists custom palettes (to /disk/theme_custom.cfg with a
// sentinel index), so always save the current theme/font selection.
fn save_theme_cfg() {
    crate::theme::save();
}

fn unsigned(value: u64, buffer: &mut [u8; 20]) -> &str {
    let mut index = buffer.len();
    let mut value = value;
    loop {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    slice_str(&buffer[index..])
}

fn signed(value: i64, buffer: &mut [u8; 20]) -> &str {
    let negative = value < 0;
    let mut magnitude = value.unsigned_abs();
    let mut index = buffer.len();
    loop {
        index -= 1;
        buffer[index] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if negative {
        index -= 1;
        buffer[index] = b'-';
    }
    slice_str(&buffer[index..])
}

fn two(value: u64, buffer: &mut [u8; 2]) {
    buffer[0] = b'0' + (value / 10) as u8 % 10;
    buffer[1] = b'0' + (value % 10) as u8;
}

fn push_str(dst: &mut [u8], used: &mut usize, src: &[u8]) {
    let take = src.len().min(dst.len() - *used);
    dst[*used..*used + take].copy_from_slice(&src[..take]);
    *used += take;
}

fn coord_x10k(value: f64) -> i64 {
    (value * 10_000.0 + if value < 0.0 { -0.5 } else { 0.5 }) as i64
}

fn location_fix() -> Option<(i64, i64, u64)> {
    let fields = k::location()?;
    if fields[3] == 0 {
        return None;
    }
    let lat = f64::from_bits(fields[0]);
    let lon = f64::from_bits(fields[1]);
    Some((coord_x10k(lat), coord_x10k(lon), fields[2]))
}

fn format_coord(x10k: i64, buffer: &mut [u8; 24]) -> &str {
    let negative = x10k < 0;
    let magnitude = x10k.unsigned_abs();
    let mut whole = magnitude / 10_000;
    let mut fraction = magnitude % 10_000;
    let mut index = buffer.len();
    for _ in 0..4 {
        index -= 1;
        buffer[index] = b'0' + (fraction % 10) as u8;
        fraction /= 10;
    }
    index -= 1;
    buffer[index] = b'.';
    loop {
        index -= 1;
        buffer[index] = b'0' + (whole % 10) as u8;
        whole /= 10;
        if whole == 0 {
            break;
        }
    }
    if negative {
        index -= 1;
        buffer[index] = b'-';
    }
    slice_str(&buffer[index..])
}

fn format_tenths(value: i64, buffer: &mut [u8; 20]) -> &str {
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let mut whole = magnitude / 10;
    let mut index = buffer.len();
    index -= 1;
    buffer[index] = b'0' + (magnitude % 10) as u8;
    index -= 1;
    buffer[index] = b'.';
    loop {
        index -= 1;
        buffer[index] = b'0' + (whole % 10) as u8;
        whole /= 10;
        if whole == 0 {
            break;
        }
    }
    if negative {
        index -= 1;
        buffer[index] = b'-';
    }
    slice_str(&buffer[index..])
}

fn round_tenths(value: i64) -> i64 {
    if value >= 0 {
        (value + 5) / 10
    } else {
        -((-value + 5) / 10)
    }
}

fn format_ms(ms: u64, buffer: &mut [u8; 12]) -> &str {
    let total = ms / 1000;
    let minutes = total / 60;
    let seconds = total % 60;
    let mut used = 0;
    let mut num = [0u8; 20];
    push_str(buffer, &mut used, unsigned(minutes, &mut num).as_bytes());
    push_str(buffer, &mut used, b":");
    if seconds < 10 {
        push_str(buffer, &mut used, b"0");
    }
    push_str(buffer, &mut used, unsigned(seconds, &mut num).as_bytes());
    slice_str(&buffer[..used])
}

fn json_find(data: &[u8], key: &[u8]) -> Option<usize> {
    if key.len() + 2 > data.len() {
        return None;
    }
    let mut i = 0;
    while i + key.len() + 2 <= data.len() {
        if data[i] == b'"'
            && &data[i + 1..i + 1 + key.len()] == key
            && data[i + 1 + key.len()] == b'"'
        {
            return Some(i + key.len() + 2);
        }
        i += 1;
    }
    None
}

fn json_num_x10(data: &[u8], key: &[u8]) -> Option<i64> {
    let mut i = json_find(data, key)?;
    while i < data.len() && (data[i] == b' ' || data[i] == b':') {
        i += 1;
    }
    let negative = i < data.len() && data[i] == b'-';
    if negative {
        i += 1;
    }
    let mut whole = 0i64;
    let mut digits = 0;
    while i < data.len() && data[i].is_ascii_digit() {
        whole = whole.saturating_mul(10).saturating_add((data[i] - b'0') as i64);
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    let mut tenth = 0i64;
    if i < data.len() && data[i] == b'.' {
        i += 1;
        if i < data.len() && data[i].is_ascii_digit() {
            tenth = (data[i] - b'0') as i64;
        }
    }
    let value = whole.saturating_mul(10).saturating_add(tenth);
    Some(if negative { -value } else { value })
}

fn weather_text(code: u64) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 | 63 | 65 => "Rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 => "Snow",
        77 => "Snow grains",
        80 | 81 | 82 => "Showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm, hail",
        _ => "Unknown",
    }
}

impl Music {
    fn set_status(&mut self, text: &str) {
        self.status_len = text.len().min(self.status.len());
        self.status[..self.status_len].copy_from_slice(&text.as_bytes()[..self.status_len]);
    }

    fn refresh(&mut self) {
        let _ = k::mkdir("/disk/music");
        self.count = 0;
        self.scroll = 0;
        let mut buffer = [0u8; 2048];
        if let Some(n) = k::listdir("/disk/music", &mut buffer) {
            for name in buffer[..n].split(|&b| b == 0) {
                if name.is_empty() || name.last() == Some(&b'/') || self.count >= 16 {
                    continue;
                }
                let lower: [u8; 48] = {
                    let mut tmp = [0u8; 48];
                    let len = name.len().min(48);
                    for i in 0..len {
                        tmp[i] = name[i].to_ascii_lowercase();
                    }
                    tmp
                };
                let len = name.len().min(48);
                let audio = lower[..len].ends_with(b".mp3")
                    || lower[..len].ends_with(b".wav")
                    || lower[..len].ends_with(b".ogg")
                    || lower[..len].ends_with(b".m4a")
                    || lower[..len].ends_with(b".flac");
                if !audio || name.len() > 47 {
                    continue;
                }
                let i = self.count;
                self.names[i][..name.len()].copy_from_slice(name);
                self.name_len[i] = name.len() as u8;
                self.count += 1;
            }
        }
        if self.track >= self.count {
            self.track = 0;
        }
        if self.scroll + 4 > self.count {
            self.scroll = self.count.saturating_sub(4);
        }
    }

    fn sync(&mut self) {
        if let Some(q) = k::audio_query() {
            self.pos_ms = q[0];
            self.dur_ms = q[1];
            self.playing = q[2] != 0;
        }
    }

    fn play_track(&mut self, index: usize) {
        if index >= self.count {
            return;
        }
        let mut path = [0u8; 80];
        let mut used = 0;
        push_str(&mut path, &mut used, b"/disk/music/");
        push_str(&mut path, &mut used, &self.names[index][..self.name_len[index] as usize]);
        if k::audio_play(slice_str(&path[..used])) {
            self.track = index;
            self.playing = true;
            self.status_len = 0;
            self.sync();
        } else {
            self.set_status("Playback failed");
        }
    }

    fn toggle(&mut self) {
        if self.playing {
            k::audio_pause();
            self.playing = false;
        } else if self.count > 0 {
            if k::audio_resume() {
                self.playing = true;
            } else {
                self.play_track(self.track);
            }
        }
        self.sync();
    }

    fn next(&mut self) {
        if self.count > 0 {
            self.play_track((self.track + 1) % self.count);
        }
    }

    fn previous(&mut self) {
        if self.count > 0 {
            self.play_track((self.track + self.count - 1) % self.count);
        }
    }
}

pub fn home_music(music: &Music) -> (&str, &str, bool, i64) {
    if music.count == 0 {
        return ("No tracks", "Import music to begin", false, 0);
    }
    let title = slice_str(&music.names[music.track][..music.name_len[music.track] as usize]);
    let (pos, dur, playing) = match k::audio_query() {
        Some(q) => (q[0], q[1], q[2] != 0),
        None => (music.pos_ms, music.dur_ms, music.playing),
    };
    let progress = if dur > 0 {
        ((pos.min(dur) * 1000 / dur) as i64).clamp(0, 1000)
    } else {
        0
    };
    (title, "/disk/music", playing, progress)
}

pub fn home_music_toggle(apps: &mut Apps) {
    apps.music.toggle();
}

pub fn home_weather(weather: &Weather) -> Option<(i64, &str)> {
    if !weather.has_data {
        return None;
    }
    let temp = if weather.fahrenheit {
        round_tenths(weather.temp * 9 / 5 + 320)
    } else {
        round_tenths(weather.temp)
    };
    let cond = if weather.cond_len > 0 {
        slice_str(&weather.cond[..weather.cond_len])
    } else {
        weather_text(weather.code)
    };
    Some((temp, cond))
}

impl Notes {
    fn set_status(&mut self, text: &str) {
        self.status_len = text.len().min(self.status.len());
        self.status[..self.status_len].copy_from_slice(&text.as_bytes()[..self.status_len]);
    }

    fn refresh(&mut self) {
        let _ = k::mkdir("/disk/notes");
        let mut buffer = [0u8; 1024];
        let mut n = k::listdir("/disk/notes", &mut buffer).unwrap_or(0);
        if buffer[..n].iter().all(|&b| b == 0) && k::stat("/disk/notes.txt").is_some() {
            let mut old = [0u8; 1024];
            if let Some(handle) = k::open("/disk/notes.txt") {
                let got = k::read_file(handle, 0, &mut old);
                k::close(handle);
                if let Some(handle) = k::create("/disk/notes/1.txt") {
                    k::write_file(handle, 0, &old[..got]);
                    k::close(handle);
                    k::unlink("/disk/notes.txt");
                }
            }
            n = k::listdir("/disk/notes", &mut buffer).unwrap_or(0);
        }
        self.count = 0;
        self.scroll = 0;
        for name in buffer[..n].split(|&b| b == 0) {
            if name.is_empty() || name.last() == Some(&b'/') || self.count >= 16 {
                continue;
            }
            if name.len() > 35 {
                continue;
            }
            let i = self.count;
            let mut used = 0;
            push_str(&mut self.paths[i], &mut used, b"/disk/notes/");
            push_str(&mut self.paths[i], &mut used, name);
            self.path_len[i] = used as u8;
            let path = slice_str(&self.paths[i][..used]);
            let mut head = [0u8; 64];
            let mut title_len = 0;
            if let Some(handle) = k::open(path) {
                let got = k::read_file(handle, 0, &mut head);
                k::close(handle);
                let end = head[..got].iter().position(|&b| b == b'\n').unwrap_or(got);
                title_len = end.min(31);
                self.titles[i][..title_len].copy_from_slice(&head[..title_len]);
            }
            if title_len == 0 {
                let base = name.strip_suffix(b".txt").unwrap_or(name);
                title_len = base.len().min(31);
                self.titles[i][..title_len].copy_from_slice(&base[..title_len]);
            }
            self.title_len[i] = title_len as u8;
            self.count += 1;
        }
    }

    fn open_edit(&mut self, index: usize) {
        if index >= self.count {
            return;
        }
        self.len = 0;
        let path = slice_str(&self.paths[index][..self.path_len[index] as usize]);
        if let Some(handle) = k::open(path) {
            self.len = k::read_file(handle, 0, &mut self.text);
            k::close(handle);
        }
        self.editing = Some(index);
        self.status_len = 0;
    }

    fn new_note(&mut self) {
        if self.count >= 16 {
            self.set_status("Note limit reached (16)");
            return;
        }
        let mut path = [0u8; 48];
        let mut created = 0;
        for number in 1..=999u64 {
            let mut num = [0u8; 20];
            let digits = unsigned(number, &mut num);
            let mut used = 0;
            push_str(&mut path, &mut used, b"/disk/notes/");
            push_str(&mut path, &mut used, digits.as_bytes());
            push_str(&mut path, &mut used, b".txt");
            let candidate = slice_str(&path[..used]);
            if k::stat(candidate).is_none() {
                if let Some(handle) = k::create(candidate) {
                    k::close(handle);
                    created = used;
                    break;
                }
            }
        }
        if created == 0 {
            self.set_status("Could not create note");
            return;
        }
        self.refresh();
        for index in 0..self.count {
            let len = self.path_len[index] as usize;
            if len == created && self.paths[index][..len] == path[..created] {
                self.open_edit(index);
                return;
            }
        }
    }

    fn update_title(&mut self, index: usize) {
        let end = self.text[..self.len].iter().position(|&b| b == b'\n').unwrap_or(self.len);
        let mut title_len = end.min(31);
        self.titles[index][..title_len].copy_from_slice(&self.text[..title_len]);
        if title_len == 0 {
            let path = &self.paths[index][..self.path_len[index] as usize];
            let base = path.strip_prefix(b"/disk/notes/").unwrap_or(path);
            let base = base.strip_suffix(b".txt").unwrap_or(base);
            title_len = base.len().min(31);
            self.titles[index][..title_len].copy_from_slice(&base[..title_len]);
        }
        self.title_len[index] = title_len as u8;
    }

    fn push(&mut self, byte: u8) {
        if self.len < self.text.len() {
            self.text[self.len] = byte;
            self.len += 1;
            self.status_len = 0;
        }
    }

    fn backspace(&mut self) {
        if self.len > 0 {
            self.len -= 1;
            self.status_len = 0;
        }
    }

    fn save(&mut self) {
        let Some(index) = self.editing else {
            return;
        };
        let path = slice_str(&self.paths[index][..self.path_len[index] as usize]);
        if let Some(handle) = k::create(path) {
            let written = k::write_file(handle, 0, &self.text[..self.len]);
            k::close(handle);
            if written == self.len {
                self.set_status("Saved");
                self.update_title(index);
            } else {
                self.set_status("Short write");
            }
        } else {
            self.set_status("No disk mounted");
        }
    }

    fn delete(&mut self) {
        let Some(index) = self.editing else {
            return;
        };
        let path = slice_str(&self.paths[index][..self.path_len[index] as usize]);
        if k::unlink(path) {
            self.set_status("Deleted");
        } else {
            self.set_status("Delete failed");
        }
        self.editing = None;
        self.refresh();
    }
}

impl Terminal {
    fn push(&mut self, text: &str) {
        for raw in text.split('\n') {
            if self.count == TERM_LINES {
                for index in 1..TERM_LINES {
                    self.lines[index - 1] = self.lines[index];
                    self.line_len[index - 1] = self.line_len[index];
                }
                self.count -= 1;
            }
            let mut used = 0usize;
            let bytes = raw.as_bytes();
            if bytes.is_empty() {
                self.line_len[self.count] = 0;
                self.count += 1;
                continue;
            }
            for &byte in bytes {
                if used == TERM_WIDTH {
                    self.line_len[self.count] = TERM_WIDTH as u8;
                    self.count += 1;
                    used = 0;
                    if self.count == TERM_LINES {
                        for index in 1..TERM_LINES {
                            self.lines[index - 1] = self.lines[index];
                            self.line_len[index - 1] = self.line_len[index];
                        }
                        self.count -= 1;
                    }
                }
                self.lines[self.count][used] = byte;
                used += 1;
            }
            self.line_len[self.count] = used as u8;
            self.count += 1;
        }
    }

    fn clear(&mut self) {
        self.count = 0;
    }
}

impl Files {
    fn set_status(&mut self, text: &str) {
        self.status_len = text.len().min(self.status.len());
        self.status[..self.status_len].copy_from_slice(&text.as_bytes()[..self.status_len]);
    }

    fn push_entry(&mut self, name: &[u8], is_dir: bool, path: &[u8], size: u64) {
        let i = self.count;
        let nl = name.len().min(47);
        self.names[i][..nl].copy_from_slice(&name[..nl]);
        self.name_len[i] = nl as u8;
        let pl = path.len().min(127);
        self.paths[i][..pl].copy_from_slice(&path[..pl]);
        self.path_len[i] = pl as u8;
        self.is_dir[i] = is_dir;
        self.sizes[i] = size;
        self.count += 1;
    }

    fn refresh(&mut self) {
        self.count = 0;
        self.preview = None;
        self.action = None;
        self.props = None;
        self.run_show = false;
        self.confirm_delete = false;
        self.scroll = 0;
        self.status_len = 0;
        let mut buffer = [0u8; 4096];
        let dir_len = self.dir_len;
        let Some(n) = k::listdir(slice_str(&self.dir[..dir_len]), &mut buffer) else {
            self.set_status("Cannot open directory");
            return;
        };
        if dir_len > 5 {
            let parent_end = self.dir[..dir_len]
                .iter()
                .rposition(|&b| b == b'/')
                .unwrap_or(0)
                .max(5);
            let mut parent = [0u8; 128];
            parent[..parent_end].copy_from_slice(&self.dir[..parent_end]);
            self.push_entry(b"..", true, &parent[..parent_end], 0);
        }
        for name in buffer[..n].split(|&b| b == 0) {
            if name.is_empty() || self.count >= 64 {
                continue;
            }
            let is_dir = name.last() == Some(&b'/');
            let base = if is_dir { &name[..name.len() - 1] } else { name };
            if base.is_empty() || base.len() > 47 || dir_len + 1 + base.len() >= 128 {
                continue;
            }
            let mut path = [0u8; 128];
            let mut used = 0;
            push_str(&mut path, &mut used, &self.dir[..dir_len]);
            push_str(&mut path, &mut used, b"/");
            push_str(&mut path, &mut used, base);
            let size = if is_dir {
                0
            } else {
                k::stat(slice_str(&path[..used])).map_or(0, |(_, size)| size)
            };
            self.push_entry(base, is_dir, &path[..used], size);
        }
        if n >= buffer.len() {
            self.set_status("Listing truncated");
        }
    }

    fn enter(&mut self, index: usize) {
        if index >= self.count {
            return;
        }
        if self.is_dir[index] {
            let len = self.path_len[index] as usize;
            self.dir[..len].copy_from_slice(&self.paths[index][..len]);
            self.dir_len = len;
            self.refresh();
        } else {
            self.open_preview(index);
        }
    }

    fn go_up(&mut self) {
        if self.dir_len <= 5 {
            return;
        }
        let end = self.dir[..self.dir_len]
            .iter()
            .rposition(|&b| b == b'/')
            .unwrap_or(0)
            .max(5);
        self.dir_len = end;
        self.refresh();
    }

    fn go_root(&mut self) {
        self.dir[..5].copy_from_slice(b"/disk");
        self.dir_len = 5;
        self.refresh();
    }

    fn open_preview(&mut self, index: usize) {
        if index >= self.count {
            return;
        }
        let path = slice_str(&self.paths[index][..self.path_len[index] as usize]);
        self.preview = Some(index);
        self.body_len = 0;
        self.binary = false;
        if let Some(handle) = k::open(path) {
            let count = k::read_file(handle, 0, &mut self.body);
            k::close(handle);
            self.body_len = count.min(self.body.len());
            for &byte in &self.body[..self.body_len] {
                if byte == 0 {
                    self.binary = true;
                    break;
                }
            }
            if !self.binary {
                for byte in &mut self.body[..self.body_len] {
                    if *byte == b'\r' || (*byte < 32 && *byte != b'\n' && *byte != b'\t') || *byte > 126 {
                        *byte = b'.';
                    }
                }
            }
        }
    }

    fn path_of(&self, index: usize) -> &str {
        slice_str(&self.paths[index][..self.path_len[index] as usize])
    }

    fn is_executable(&self, index: usize) -> bool {
        if index >= self.count || self.is_dir[index] {
            return false;
        }
        let path = self.path_of(index);
        if path.starts_with("/disk/bin/") {
            return true;
        }
        let mut magic = [0u8; 4];
        if let Some(h) = k::open(path) {
            let n = k::read_file(h, 0, &mut magic);
            k::close(h);
            return n == 4 && &magic == b"\x7fELF";
        }
        false
    }

    fn close_overlays(&mut self) {
        self.action = None;
        self.props = None;
        self.run_show = false;
        self.confirm_delete = false;
        self.prompt = 0;
        self.prompt_len = 0;
    }

    fn start_prompt(&mut self, kind: u8, index: Option<usize>) {
        self.prompt = kind;
        self.prompt_len = 0;
        if kind == 1 {
            if let Some(i) = index {
                let name = &self.names[i][..self.name_len[i] as usize];
                let len = name.len().min(48);
                self.prompt_buf[..len].copy_from_slice(name);
                self.prompt_len = len;
            }
        }
    }

    fn prompt_push(&mut self, byte: u8) {
        if self.prompt_len < self.prompt_buf.len() {
            self.prompt_buf[self.prompt_len] = byte;
            self.prompt_len += 1;
        }
    }

    fn prompt_confirm(&mut self) {
        if self.prompt_len == 0 {
            self.set_status("Enter a name first");
            return;
        }
        let mut target = [0u8; 176];
        let mut used = 0;
        push_str(&mut target, &mut used, &self.dir[..self.dir_len]);
        push_str(&mut target, &mut used, b"/");
        push_str(&mut target, &mut used, &self.prompt_buf[..self.prompt_len]);
        let target_str = slice_str(&target[..used]);
        let kind = self.prompt;
        let ok = match kind {
            1 => {
                let Some(i) = self.action else {
                    self.prompt = 0;
                    return;
                };
                k::rename(self.path_of(i), target_str)
            }
            2 => match k::create(target_str) {
                Some(h) => {
                    k::close(h);
                    true
                }
                None => false,
            },
            3 => k::mkdir(target_str),
            _ => false,
        };
        self.set_status(if ok {
            match kind { 1 => "Renamed", 2 => "File created", _ => "Folder created" }
        } else {
            "Operation failed (name in use?)"
        });
        self.prompt = 0;
        self.prompt_len = 0;
        self.action = None;
        self.refresh();
    }

    fn open_editor(&mut self, index: usize) {
        if index >= self.count || self.is_dir[index] {
            return;
        }
        self.edit_len = 0;
        if let Some(h) = k::open(self.path_of(index)) {
            self.edit_len = k::read_file(h, 0, &mut self.edit);
            k::close(h);
        }
        self.editor = Some(index);
        self.preview = None;
        self.action = None;
        self.status_len = 0;
    }

    fn editor_save(&mut self) {
        let Some(index) = self.editor else { return };
        if index >= self.count {
            self.editor = None;
            return;
        }
        let written = match k::create(self.path_of(index)) {
            Some(h) => {
                let w = k::write_file(h, 0, &self.edit[..self.edit_len]);
                k::close(h);
                Some(w)
            }
            None => None,
        };
        self.set_status(match written {
            Some(w) if w == self.edit_len => "Saved",
            Some(_) => "Short write",
            None => "Save failed",
        });
    }

    fn clipboard_set(&mut self, index: usize, cut: bool) {
        if index >= self.count || self.names[index][..self.name_len[index] as usize] == b".."[..] {
            return;
        }
        let pl = self.path_len[index] as usize;
        self.clip_path[..pl].copy_from_slice(&self.paths[index][..pl]);
        self.clip_len = pl;
        let nl = self.name_len[index] as usize;
        self.clip_name[..nl].copy_from_slice(&self.names[index][..nl]);
        self.clip_name_len = nl;
        self.clip_cut = cut;
        self.action = None;
        self.set_status(if cut { "Move: navigate, then Paste" } else { "Copied: navigate, then Paste" });
    }

    fn paste(&mut self) {
        if self.clip_len == 0 {
            return;
        }
        let mut dst = [0u8; 176];
        let mut used = 0;
        push_str(&mut dst, &mut used, &self.dir[..self.dir_len]);
        push_str(&mut dst, &mut used, b"/");
        push_str(&mut dst, &mut used, &self.clip_name[..self.clip_name_len]);
        let dst_str = slice_str(&dst[..used]);
        if k::stat(dst_str).is_some() {
            self.set_status("Destination already exists");
            return;
        }
        let src = slice_str(&self.clip_path[..self.clip_len]);
        let ok = if self.clip_cut {
            k::rename(src, dst_str)
        } else {
            k::copy_file(src, dst_str)
        };
        self.set_status(if ok {
            if self.clip_cut { "Moved" } else { "Pasted" }
        } else {
            "Paste failed"
        });
        if ok {
            self.clip_len = 0;
            self.clip_name_len = 0;
        }
        self.refresh();
    }

    fn delete_entry(&mut self, index: usize) {
        if index >= self.count {
            return;
        }
        let ok = if self.is_dir[index] {
            k::rmdir(self.path_of(index))
        } else {
            k::unlink(self.path_of(index))
        };
        self.set_status(if ok { "Deleted" } else { "Delete failed (dir not empty?)" });
        self.action = None;
        self.confirm_delete = false;
        self.refresh();
    }

    fn run_entry(&mut self, index: usize) {
        if index >= self.count {
            return;
        }
        let code = k::run_path(self.path_of(index), "");
        self.run_len = 0;
        match code {
            Some(code) => {
                push_str(&mut self.run_msg, &mut self.run_len, b"Exited with code ");
                let mut num = [0u8; 20];
                push_str(&mut self.run_msg, &mut self.run_len, unsigned(code, &mut num).as_bytes());
                push_str(&mut self.run_msg, &mut self.run_len, b" (output on console)");
            }
            None => {
                push_str(&mut self.run_msg, &mut self.run_len, b"Run unavailable on this host");
            }
        }
        self.run_show = true;
        self.action = None;
        self.preview = None;
    }

    fn export_entry(&mut self, index: usize) {
        if index >= self.count || self.is_dir[index] {
            self.set_status("Only files can be exported");
            return;
        }
        let _ = k::mkdir("/disk/export");
        let mut dst = [0u8; 160];
        let mut used = 0;
        push_str(&mut dst, &mut used, b"/disk/export/");
        push_str(&mut dst, &mut used, &self.names[index][..self.name_len[index] as usize]);
        let ok = k::copy_file(self.path_of(index), slice_str(&dst[..used]));
        self.set_status(if ok { "Exported to /disk/export (SAF share TODO)" } else { "Export failed" });
        self.action = None;
    }
}

impl Maps {
    fn set_status(&mut self, text: &str) {
        self.status_len = text.len().min(self.status.len());
        self.status[..self.status_len].copy_from_slice(&text.as_bytes()[..self.status_len]);
    }

    fn refresh(&mut self) {
        match location_fix() {
            Some((lat, lon, accuracy)) => {
                self.has_fix = true;
                self.lat = lat;
                self.lon = lon;
                self.accuracy = accuracy;
                self.set_status("Location fix acquired");
            }
            None => {
                self.has_fix = false;
                self.set_status("Location unavailable");
            }
        }
    }

    fn open_live(&mut self) {
        self.refresh();
        if !self.has_fix {
            return;
        }
        let mut url = [0u8; 128];
        let mut used = 0;
        push_str(&mut url, &mut used, b"https://www.openstreetmap.org/#map=15/");
        let mut coord = [0u8; 24];
        push_str(&mut url, &mut used, format_coord(self.lat, &mut coord).as_bytes());
        push_str(&mut url, &mut used, b"/");
        push_str(&mut url, &mut used, format_coord(self.lon, &mut coord).as_bytes());
        if k::open_url(slice_str(&url[..used])) {
            self.set_status("Opened in browser");
        } else {
            self.set_status("WebView unavailable");
        }
    }
}

impl Weather {
    fn set_status(&mut self, text: &str) {
        self.status_len = text.len().min(self.status.len());
        self.status[..self.status_len].copy_from_slice(&text.as_bytes()[..self.status_len]);
    }

    fn refresh(&mut self) {
        let Some((lat, lon, accuracy)) = location_fix() else {
            self.has_fix = false;
            self.set_status("Location unavailable");
            return;
        };
        self.has_fix = true;
        self.lat = lat;
        self.lon = lon;
        self.accuracy = accuracy;
        let hk = (221_500..=226_000).contains(&lat) && (1_138_000..=1_144_500).contains(&lon);
        let mut body = [0u8; 8192];
        if hk {
            if k::fetch_url(
                "https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=rhrread&lang=en",
                "/disk/weather.json",
            )
            .is_none()
            {
                self.set_status("Fetch failed - check network");
                return;
            }
            let n = read_disk_file("/disk/weather.json", &mut body);
            if !parse_hko(self, &body[..n]) {
                self.has_data = false;
                self.set_status("Bad weather data");
                return;
            }
            self.forecast_count = 0;
            if k::fetch_url(
                "https://data.weather.gov.hk/weatherAPI/opendata/weather.php?dataType=fnd&lang=en",
                "/disk/weather_fnd.json",
            )
            .is_some()
            {
                let n = read_disk_file("/disk/weather_fnd.json", &mut body);
                parse_fnd(self, &body[..n]);
            }
            self.has_data = true;
            self.set_status("Live from HKO");
        } else {
            let mut url = [0u8; 256];
            let mut used = 0;
            push_str(&mut url, &mut used, b"https://api.open-meteo.com/v1/forecast?latitude=");
            let mut coord = [0u8; 24];
            push_str(&mut url, &mut used, format_coord(lat, &mut coord).as_bytes());
            push_str(&mut url, &mut used, b"&longitude=");
            push_str(&mut url, &mut used, format_coord(lon, &mut coord).as_bytes());
            push_str(
                &mut url,
                &mut used,
                b"&current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m&timezone=auto",
            );
            if k::fetch_url(slice_str(&url[..used]), "/disk/weather.json").is_none() {
                self.set_status("Fetch failed - check network");
                return;
            }
            let n = read_disk_file("/disk/weather.json", &mut body);
            if !parse_open_meteo(self, &body[..n]) {
                self.has_data = false;
                self.set_status("Bad weather data");
                return;
            }
            self.forecast_count = 0;
            self.has_data = true;
            self.set_status("Live from Open-Meteo");
        }
    }
}

fn read_disk_file(path: &str, buf: &mut [u8]) -> usize {
    let Some(handle) = k::open(path) else {
        return 0;
    };
    let n = k::read_file(handle, 0, buf);
    k::close(handle);
    n
}

fn json_str(data: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut i = json_find(data, key)?;
    while i < data.len() && (data[i] == b' ' || data[i] == b':') {
        i += 1;
    }
    if i >= data.len() || data[i] != b'"' {
        return None;
    }
    i += 1;
    let mut n = 0;
    while i < data.len() && data[i] != b'"' {
        if n < out.len() {
            out[n] = data[i];
            n += 1;
        }
        i += 1;
    }
    Some(n)
}

fn json_str_is(data: &[u8], key: &[u8], want: &[u8]) -> bool {
    let mut buf = [0u8; 48];
    match json_str(data, key, &mut buf) {
        Some(n) => &buf[..n] == want,
        None => false,
    }
}

fn json_int(data: &[u8], key: &[u8]) -> Option<i64> {
    let mut i = json_find(data, key)?;
    while i < data.len() && matches!(data[i], b' ' | b'\t' | b'\r' | b'\n' | b':' | b'[') {
        i += 1;
    }
    let negative = i < data.len() && data[i] == b'-';
    if negative {
        i += 1;
    }
    let mut value = 0i64;
    let mut digits = 0;
    while i < data.len() && data[i].is_ascii_digit() {
        value = value.saturating_mul(10).saturating_add((data[i] - b'0') as i64);
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(if negative { -value } else { value })
}

fn hko_place_value(data: &[u8], section: &[u8], place: &[u8]) -> Option<i64> {
    let start = json_find(data, section)?;
    let mut i = start + json_find(&data[start..], b"data")?;
    loop {
        while i < data.len() && matches!(data[i], b' ' | b'\t' | b'\r' | b'\n' | b':' | b',' | b'[') {
            i += 1;
        }
        if i >= data.len() || data[i] != b'{' {
            return None;
        }
        let close = data[i..].iter().position(|&b| b == b'}')?;
        let obj = &data[i..i + close];
        if json_str_is(obj, b"place", place) {
            return json_num_x10(obj, b"value");
        }
        i += close + 1;
    }
}

fn json_object_at(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i < data.len() && data[i] != b'{' {
        if data[i] == b']' {
            return None;
        }
        i += 1;
    }
    if i >= data.len() {
        return None;
    }
    let start = i;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escape = false;
    while i < data.len() {
        let c = data[i];
        if in_str {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some((start, i + 1));
            }
        }
        i += 1;
    }
    None
}

fn hko_icon_text(code: u64) -> &'static str {
    match code {
        50 => "Sunny",
        51 => "Sunny Periods",
        52 => "Sunny Intervals",
        53 => "Sunny, Few Showers",
        54 => "Sunny, Showers",
        60 => "Cloudy",
        61 => "Overcast",
        62 => "Light Rain",
        63 => "Rain",
        64 => "Heavy Rain",
        65 => "Thunderstorms",
        70..=75 => "Fine",
        76 => "Mainly Cloudy",
        77 => "Mainly Fine",
        80 => "Windy",
        81 => "Dry",
        82 => "Humid",
        83 => "Fog",
        84 => "Mist",
        85 => "Haze",
        90 => "Hot",
        91 => "Warm",
        92 => "Cool",
        93 => "Cold",
        _ => "Unknown",
    }
}

fn parse_hko(weather: &mut Weather, data: &[u8]) -> bool {
    let Some(temp) = hko_place_value(data, b"temperature", b"Hong Kong Observatory") else {
        return false;
    };
    weather.temp = temp;
    weather.humidity = hko_place_value(data, b"humidity", b"Hong Kong Observatory")
        .map(|v| v / 10)
        .unwrap_or(-1);
    weather.feels = FEELS_NONE;
    weather.wind = -1;
    weather.code = 0;
    weather.provider = 1;
    let icon = json_int(data, b"icon").unwrap_or(0).max(0) as u64;
    weather.set_cond(hko_icon_text(icon));
    true
}

fn parse_open_meteo(weather: &mut Weather, data: &[u8]) -> bool {
    let Some(start) = json_find(data, b"current") else {
        return false;
    };
    let Some(open) = data[start..].iter().position(|&b| b == b'{') else {
        return false;
    };
    let obj_start = start + open;
    let Some(close) = data[obj_start..].iter().position(|&b| b == b'}') else {
        return false;
    };
    let obj = &data[obj_start..obj_start + close];
    let Some(temp) = json_num_x10(obj, b"temperature_2m") else {
        return false;
    };
    weather.temp = temp;
    weather.humidity = json_num_x10(obj, b"relative_humidity_2m")
        .map(|v| v / 10)
        .unwrap_or(-1);
    weather.feels = json_num_x10(obj, b"apparent_temperature").unwrap_or(FEELS_NONE);
    weather.wind = json_num_x10(obj, b"wind_speed_10m").unwrap_or(-1);
    weather.code = (json_num_x10(obj, b"weather_code").unwrap_or(0) / 10).max(0) as u64;
    weather.provider = 2;
    weather.set_cond(weather_text(weather.code));
    true
}

fn parse_fnd(weather: &mut Weather, data: &[u8]) {
    weather.forecast_count = 0;
    let Some(start) = json_find(data, b"weatherForecast") else {
        return;
    };
    let mut i = start;
    while weather.forecast_count < 9 {
        let Some((s, e)) = json_object_at(data, i) else {
            break;
        };
        let obj = &data[s..e];
        let mut day = ForecastDay { week: [b' '; 3], icon: 0, max: 0, min: 0 };
        let mut week = [0u8; 16];
        if let Some(n) = json_str(obj, b"week", &mut week) {
            for j in 0..3 {
                day.week[j] = if j < n { week[j].to_ascii_uppercase() } else { b' ' };
            }
        }
        day.icon = json_int(obj, b"ForecastIcon").unwrap_or(0).max(0) as u64;
        if let Some(p) = json_find(obj, b"forecastMaxtemp") {
            day.max = json_num_x10(&obj[p..], b"value").unwrap_or(0);
        }
        if let Some(p) = json_find(obj, b"forecastMintemp") {
            day.min = json_num_x10(&obj[p..], b"value").unwrap_or(0);
        }
        weather.forecast[weather.forecast_count] = day;
        weather.forecast_count += 1;
        i = e;
    }
}

impl Apps {
    pub fn new() -> Self {
        let mut terminal = Terminal {
            lines: [[0u8; TERM_WIDTH]; TERM_LINES],
            line_len: [0u8; TERM_LINES],
            count: 0,
            input: [0u8; 64],
            input_len: 0,
        };
        terminal.push("KimiOS terminal 0.1");
        terminal.push("Type help for commands.");
        Self {
            music: Music {
                track: 0,
                count: 0,
                names: [[0u8; 48]; 16],
                name_len: [0u8; 16],
                playing: false,
                pos_ms: 0,
                dur_ms: 0,
                scroll: 0,
                status: [0u8; 48],
                status_len: 0,
            },
            notes: Notes {
                text: [0u8; 1024],
                len: 0,
                editing: None,
                count: 0,
                titles: [[0u8; 32]; 16],
                title_len: [0u8; 16],
                paths: [[0u8; 48]; 16],
                path_len: [0u8; 16],
                scroll: 0,
                status: [0u8; 48],
                status_len: 0,
            },
            maps: Maps {
                pin: None,
                has_fix: false,
                lat: 0,
                lon: 0,
                accuracy: 0,
                status: [0u8; 64],
                status_len: 0,
            },
            photos: Photos { selected: None },
            files: Files {
                dir: {
                    let mut dir = [0u8; 128];
                    dir[..5].copy_from_slice(b"/disk");
                    dir
                },
                dir_len: 5,
                count: 0,
                names: [[0u8; 48]; 64],
                name_len: [0u8; 64],
                paths: [[0u8; 128]; 64],
                path_len: [0u8; 64],
                sizes: [0u64; 64],
                is_dir: [false; 64],
                scroll: 0,
                preview: None,
                body: [0u8; 768],
                body_len: 0,
                binary: false,
                status: [0u8; 48],
                status_len: 0,
                action: None,
                confirm_delete: false,
                prompt: 0,
                prompt_buf: [0u8; 48],
                prompt_len: 0,
                editor: None,
                edit: [0u8; 2048],
                edit_len: 0,
                clip_path: [0u8; 128],
                clip_len: 0,
                clip_name: [0u8; 48],
                clip_name_len: 0,
                clip_cut: false,
                props: None,
                run_show: false,
                run_msg: [0u8; 64],
                run_len: 0,
            },
            weather: Weather::new(),
            terminal,
            browser: Browser::new(),
            apk: ApkApp {
                names: [[0; 32]; 8],
                name_lens: [0; 8],
                count: 0,
                selected: None,
                info: k::apk::ApkInfo::new(),
            },
            dvm: DvmApp {
                names: [[0; 32]; 8],
                name_lens: [0; 8],
                count: 0,
                selected: None,
                output: [0; 1024],
                out_len: 0,
                ok: false,
                err: [0; 48],
                err_len: 0,
            },
            market: Market::new(),
            monitor: Monitor::new(),
            plugins: Plugins::new(),
            plugin_home: PluginLauncher {
                items: [EMPTY_PLUGIN; 16],
                count: 0,
            },
        }
    }
}

pub fn open(apps: &mut Apps, app: AppKind) {
    match app {
        AppKind::Music => apps.music.refresh(),
        AppKind::Notes => apps.notes.refresh(),
        AppKind::Maps => apps.maps.refresh(),
        AppKind::Weather => apps.weather.refresh(),
        AppKind::Files => apps.files.refresh(),
        AppKind::Browser => apps.browser.open(),
        AppKind::Apk => apps.apk.refresh(),
        AppKind::Dvm => apps.dvm.refresh(),
        AppKind::Market => apps.market.refresh(),
        AppKind::Monitor => apps.monitor.refresh(),
        AppKind::Plugins => apps.plugins.refresh(),
        _ => {}
    }
}

/// Long-press hook (shell detects the hold). Returns true when consumed.
pub fn long_press(apps: &mut Apps, app: AppKind, point: (i64, i64)) -> bool {
    match app {
        AppKind::Files => files_long_press(apps, point),
        _ => false,
    }
}

pub fn back(apps: &mut Apps, app: AppKind) -> bool {
    match app {
        AppKind::Browser => false,
        AppKind::Notes => false,
        AppKind::Apk => {
            if apps.apk.selected.take().is_some() {
                true
            } else {
                false
            }
        }
        AppKind::Dvm => {
            if apps.dvm.selected.take().is_some() {
                true
            } else {
                false
            }
        }
        AppKind::Photos => {
            if apps.photos.selected.take().is_some() {
                true
            } else {
                false
            }
        }
        AppKind::Files => apps.files.preview.take().is_some(),
        _ => false,
    }
}

/// Back-arrow navigation: step back one screen inside the app. Returns false
/// when there is nothing left to unwind (the shell then closes the app).
pub fn nav_back(apps: &mut Apps, app: AppKind) -> bool {
    match app {
        AppKind::Notes => {
            if apps.notes.editing.is_some() {
                apps.notes.save();
                apps.notes.editing = None;
                true
            } else {
                false
            }
        }
        AppKind::Files => {
            let files = &mut apps.files;
            if files.prompt != 0 {
                files.prompt = 0;
                files.prompt_len = 0;
                files.action = None;
                true
            } else if files.editor.is_some() {
                files.editor_save();
                files.editor = None;
                files.refresh();
                true
            } else if files.run_show {
                files.run_show = false;
                true
            } else if files.props.take().is_some() {
                true
            } else if files.action.take().is_some() {
                files.confirm_delete = false;
                true
            } else if files.preview.take().is_some() {
                true
            } else if files.dir_len > 5 {
                files.go_up();
                true
            } else {
                false
            }
        }
        AppKind::Photos => apps.photos.selected.take().is_some(),
        AppKind::Apk => apps.apk.selected.take().is_some(),
        AppKind::Dvm => apps.dvm.selected.take().is_some(),
        AppKind::Monitor => apps.monitor.selected.take().is_some(),
        AppKind::Plugins => {
            let plugins = &mut apps.plugins;
            if plugins.confirm_uninstall {
                plugins.confirm_uninstall = false;
                true
            } else if plugins.view != 0 {
                plugins.view = 0;
                true
            } else {
                plugins.selected.take().is_some()
            }
        }
        _ => false,
    }
}

fn key_event(code: u16, shift: bool) -> Option<(u8, bool)> {
    let value = match code {
        2..=11 => {
            let digit = b"1234567890"[(code - 2) as usize];
            (if shift { b"!@#$%^&*()"[(code - 2) as usize] } else { digit }, false)
        }
        12 => (if shift { b'_' } else { b'-' }, false),
        13 => (if shift { b'+' } else { b'=' }, false),
        14 => (0, true),
        16..=25 => {
            let letter = b"qwertyuiop"[(code - 16) as usize];
            (if shift { letter.to_ascii_uppercase() } else { letter }, false)
        }
        26 => (if shift { b'{' } else { b'[' }, false),
        27 => (if shift { b'}' } else { b']' }, false),
        28 => (b'\n', false),
        30..=38 => {
            let letter = b"asdfghjkl"[(code - 30) as usize];
            (if shift { letter.to_ascii_uppercase() } else { letter }, false)
        }
        39 => (if shift { b':' } else { b';' }, false),
        40 => (if shift { b'"' } else { b'\'' }, false),
        41 => (if shift { b'~' } else { b'`' }, false),
        43 => (if shift { b'|' } else { b'\\' }, false),
        44..=50 => {
            let letter = b"zxcvbnm"[(code - 44) as usize];
            (if shift { letter.to_ascii_uppercase() } else { letter }, false)
        }
        51 => (if shift { b'<' } else { b',' }, false),
        52 => (if shift { b'>' } else { b'.' }, false),
        53 => (if shift { b'?' } else { b'/' }, false),
        57 => (b' ', false),
        _ => return None,
    };
    Some(value)
}

/// Printable byte a keycode produces (None for backspace/unknown). Used by
/// the on-screen keyboard to label its keys from the same mapping.
pub fn key_glyph(code: u16, shift: bool) -> Option<u8> {
    match key_event(code, shift) {
        Some((0, true)) => None,
        Some((byte, _)) => Some(byte),
        None => None,
    }
}

/// Whether the app currently has a focused text field (OSK visibility gate).
pub fn text_focused(apps: &Apps, app: AppKind) -> bool {
    match app {
        AppKind::Notes => apps.notes.editing.is_some(),
        AppKind::Terminal => true,
        AppKind::Browser => apps.browser.editing,
        AppKind::Files => apps.files.editor.is_some() || apps.files.prompt != 0,
        _ => false,
    }
}

/// Whether an app-space tap lands on the text field itself (reopens the OSK).
pub fn field_hit(apps: &Apps, app: AppKind, point: (i64, i64), _osk: bool) -> bool {
    match app {
        AppKind::Notes => {
            apps.notes.editing.is_some()
                && point.0 >= 20 && point.0 <= 460 && point.1 >= 160 && point.1 <= 614
        }
        AppKind::Terminal => point.1 > 96 && point.1 < 770,
        AppKind::Browser => point.0 >= 20 && point.0 < 390 && point.1 >= 104 && point.1 <= 148,
        AppKind::Files => {
            (apps.files.editor.is_some() && point.1 >= 160 && point.1 <= 614)
                || (apps.files.prompt != 0 && point.1 >= 300 && point.1 <= 480)
        }
        _ => false,
    }
}

/// Printable byte (or backspace marker) a keycode produces, for the home
/// search field which lives outside the Apps dispatch.
pub fn key_char(code: u16, shift: bool) -> Option<(u8, bool)> {
    key_event(code, shift)
}

pub fn key(apps: &mut Apps, app: AppKind, code: u16, shift: bool) -> bool {
    match app {
        AppKind::Files => files_key(apps, code, shift),
        AppKind::Plugins => plugins_key(apps, code),
        AppKind::Notes => {
            if apps.notes.editing.is_none() {
                return false;
            }
            match key_event(code, shift) {
                Some((0, true)) => {
                    apps.notes.backspace();
                    true
                }
                Some((byte, _)) => {
                    apps.notes.push(byte);
                    true
                }
                None => false,
            }
        }
        AppKind::Terminal => {
            match key_event(code, shift) {
                Some((0, true)) => {
                    if apps.terminal.input_len > 0 {
                        apps.terminal.input_len -= 1;
                    }
                    true
                }
                Some((b'\n', _)) => {
                    terminal_execute(apps);
                    true
                }
                Some((byte, _)) => {
                    if apps.terminal.input_len < apps.terminal.input.len() {
                        let index = apps.terminal.input_len;
                        apps.terminal.input[index] = byte;
                        apps.terminal.input_len += 1;
                    }
                    true
                }
                None => false,
            }
        }
        AppKind::Browser => {
            if !apps.browser.editing {
                match code {
                    103 => {
                        apps.browser.scroll = apps.browser.scroll.saturating_sub(5);
                        true
                    }
                    108 => {
                        if apps.browser.scroll + 34 < apps.browser.line_count {
                            apps.browser.scroll += 5;
                        }
                        true
                    }
                    _ => false,
                }
            } else {
                match key_event(code, shift) {
                    Some((0, true)) => {
                        apps.browser.url_len = apps.browser.url_len.saturating_sub(1);
                        true
                    }
                    Some((b'\n', _)) => {
                        browser_go(apps);
                        true
                    }
                    Some((byte, _)) => {
                        let browser = &mut apps.browser;
                        if browser.url_len < browser.url.len() {
                            let index = browser.url_len;
                            browser.url[index] = byte;
                            browser.url_len += 1;
                        }
                        true
                    }
                    None => false,
                }
            }
        }
        _ => false,
    }
}

fn terminal_execute(apps: &mut Apps) {
    let mut command = [0u8; 64];
    let length = apps.terminal.input_len.min(64);
    command[..length].copy_from_slice(&apps.terminal.input[..length]);
    apps.terminal.input_len = 0;
    let mut start = 0;
    let mut end = length;
    while start < end && command[start] == b' ' {
        start += 1;
    }
    while end > start && command[end - 1] == b' ' {
        end -= 1;
    }
    let line = slice_str(&command[start..end]);
    let mut echo = [0u8; 70];
    echo[0] = b'$';
    echo[1] = b' ';
    let shown = (end - start).min(63);
    echo[2..2 + shown].copy_from_slice(&command[start..start + shown]);
    apps.terminal.push(slice_str(&echo[..2 + shown]));
    if line.is_empty() {
        return;
    }
    let mut split = 0;
    while split < line.len() && line.as_bytes()[split] != b' ' {
        split += 1;
    }
    let (name, _rest) = line.split_at(split);
    match name {
        "" => {}
        "clear" => apps.terminal.clear(),
        "exit" => apps.terminal.push("Close with the back arrow."),
        "exec" => apps.terminal.push("exec: use 'run <file>' in the GUI terminal"),
        _ => {
            // Same command interpreter as the CLI shell; capture its console
            // output and render it into the terminal display.
            k::capture_begin();
            k::shell::exec(line.as_bytes());
            let mut out = [0u8; 8192];
            let n = k::capture_take(&mut out);
            k::capture_end();
            for byte in &mut out[..n] {
                if *byte == b'\n' || *byte == b'\t' {
                    continue;
                }
                if *byte < 32 || *byte > 126 {
                    *byte = b'.';
                }
            }
            if n > 0 {
                apps.terminal.push(slice_str(&out[..n]));
            }
        }
    }
}

fn format_date(clock: [u64; 7], buffer: &mut [u8; 20]) -> &str {
    let mut year = [0u8; 20];
    let text = unsigned(clock[1], &mut year);
    let mut used = 0;
    for &byte in text.as_bytes() {
        buffer[used] = byte;
        used += 1;
    }
    buffer[used] = b'-';
    used += 1;
    let mut month = [0u8; 2];
    two(clock[2].clamp(1, 12), &mut month);
    buffer[used] = month[0];
    buffer[used + 1] = month[1];
    used += 2;
    buffer[used] = b'-';
    used += 1;
    let mut day = [0u8; 2];
    two(clock[3].clamp(1, 31), &mut day);
    buffer[used] = day[0];
    buffer[used + 1] = day[1];
    used += 2;
    buffer[used] = b' ';
    used += 1;
    let mut hour = [0u8; 2];
    two(clock[4] % 24, &mut hour);
    buffer[used] = hour[0];
    buffer[used + 1] = hour[1];
    used += 2;
    buffer[used] = b':';
    used += 1;
    let mut minute = [0u8; 2];
    two(clock[5] % 60, &mut minute);
    buffer[used] = minute[0];
    buffer[used + 1] = minute[1];
    used += 2;
    buffer[used] = b':';
    used += 1;
    let mut second = [0u8; 2];
    two(clock[6] % 60, &mut second);
    buffer[used] = second[0];
    buffer[used + 1] = second[1];
    used += 2;
    slice_str(&buffer[..used])
}

fn button(screen: &Screen, x: i64, y: i64, width: i64, height: i64, label: &str, color: u32, text_color: u32) {
    screen.round_rect(x, y, width, height, height / 2, color);
    screen.text_centered(x + width / 2, y + height / 2 - 8, label, 2, text_color);
}

pub fn tap(screen: &Screen, apps: &mut Apps, app: AppKind, point: (i64, i64), osk: bool, _clock: [u64; 7]) {
    match app {
        AppKind::Music => music_tap(apps, point),
        AppKind::Notes => notes_tap(apps, point),
        AppKind::Maps => maps_tap(apps, point),
        AppKind::Photos => photos_tap(apps, point),
        AppKind::Files => files_tap(apps, point),
        AppKind::Weather => weather_tap(apps, point),
        AppKind::Browser => browser_tap(apps, point, osk),
        AppKind::Apk => apk_tap(apps, point),
        AppKind::Dvm => dvm_tap(apps, point),
        AppKind::Market => market_tap(apps, point),
        AppKind::Monitor => monitor_tap(apps, point),
        AppKind::Plugins => plugins_tap(screen, apps, point),
        _ => {}
    }
}

fn music_tap(apps: &mut Apps, point: (i64, i64)) {
    let music = &mut apps.music;
    if point.1 >= 104 && point.1 <= 144 {
        if point.0 >= 236 && point.0 <= 346 {
            if k::import_file("/disk/music") {
                music.set_status("Import started - rescan after");
            } else {
                music.set_status("Import unavailable");
            }
            return;
        }
        if point.0 >= 352 && point.0 <= 456 {
            music.refresh();
            return;
        }
    }
    let dy = point.1 - 470;
    let dx = point.0 - 240;
    if dx * dx + dy * dy <= 42 * 42 {
        music.toggle();
        return;
    }
    let dx = point.0 - 140;
    if dx * dx + dy * dy <= 32 * 32 {
        music.previous();
        return;
    }
    let dx = point.0 - 340;
    if dx * dx + dy * dy <= 32 * 32 {
        music.next();
        return;
    }
    if point.1 >= 516 && point.1 <= 544 {
        if point.0 >= 368 && point.0 <= 408 {
            music.scroll = music.scroll.saturating_sub(1);
            return;
        }
        if point.0 >= 416 && point.0 <= 456 {
            if music.scroll + 4 < music.count {
                music.scroll += 1;
            }
            return;
        }
    }
    if point.1 >= 548 && point.1 < 756 && point.0 >= 20 && point.0 < 460 {
        let row = ((point.1 - 548) / 52) as usize;
        if row < 4 {
            let index = music.scroll + row;
            if index < music.count {
                music.play_track(index);
            }
        }
    }
}

fn notes_tap(apps: &mut Apps, point: (i64, i64)) {
    let notes = &mut apps.notes;
    if notes.editing.is_some() {
        if point.1 >= 104 && point.1 <= 148 {
            if point.0 >= 236 && point.0 <= 340 {
                notes.save();
            } else if point.0 >= 348 && point.0 <= 456 {
                notes.delete();
            }
        }
        return;
    }
    if point.1 >= 104 && point.1 <= 148 && point.0 >= 356 && point.0 <= 456 {
        notes.new_note();
        return;
    }
    if point.1 >= 728 && point.1 <= 764 {
        if point.0 >= 20 && point.0 <= 140 {
            notes.scroll = notes.scroll.saturating_sub(1);
        } else if point.0 >= 340 && point.0 <= 460 {
            if notes.scroll + 10 < notes.count {
                notes.scroll += 1;
            }
        }
        return;
    }
    if point.1 >= 160 && point.1 < 720 && point.0 >= 20 && point.0 < 460 {
        let row = ((point.1 - 160) / 56) as usize;
        if row < 10 {
            let index = notes.scroll + row;
            if index < notes.count {
                notes.open_edit(index);
            }
        }
    }
}

fn maps_tap(apps: &mut Apps, point: (i64, i64)) {
    if apps.maps.has_fix
        && point.0 >= 296 && point.0 <= 440 && point.1 >= 656 && point.1 <= 700
    {
        apps.maps.open_live();
        return;
    }
    if point.1 >= 120 && point.1 <= 620 && point.0 >= 20 && point.0 <= 460 {
        apps.maps.pin = Some((point.0, point.1));
    }
}

fn photos_tap(apps: &mut Apps, point: (i64, i64)) {
    if apps.photos.selected.is_some() {
        apps.photos.selected = None;
        return;
    }
    for index in 0..6usize {
        let column = index as i64 % 2;
        let row = index as i64 / 2;
        let x = 24 + column * 224;
        let y = 120 + row * 156;
        if point.0 >= x && point.0 < x + 208 && point.1 >= y && point.1 < y + 144 {
            apps.photos.selected = Some(index);
            return;
        }
    }
}

fn files_row_at(point: (i64, i64)) -> Option<usize> {
    if point.1 >= 196 && point.1 < 706 && point.0 >= 20 && point.0 < 460 {
        let row = ((point.1 - 196) / 30) as usize;
        if row < 17 {
            return Some(row);
        }
    }
    None
}

fn files_tap(apps: &mut Apps, point: (i64, i64)) {
    let files = &mut apps.files;
    if files.prompt != 0 {
        if point.1 >= 420 && point.1 <= 464 {
            if point.0 >= 240 && point.0 <= 336 {
                files.prompt_confirm();
            } else if point.0 >= 348 && point.0 <= 444 {
                files.prompt = 0;
                files.prompt_len = 0;
                files.action = None;
            }
        }
        return;
    }
    if files.editor.is_some() {
        if point.1 >= 104 && point.1 <= 148 {
            if point.0 >= 236 && point.0 <= 340 {
                files.editor_save();
            } else if point.0 >= 348 && point.0 <= 456 {
                files.editor_save();
                files.editor = None;
                files.refresh();
            }
        }
        return;
    }
    if files.run_show {
        files.run_show = false;
        return;
    }
    if files.props.is_some() {
        files.props = None;
        return;
    }
    if let Some(index) = files.action {
        if point.1 >= 216 && point.1 <= 560 && point.0 >= 56 && point.0 <= 424 {
            let col = if point.0 < 240 { 0 } else { 1 };
            let row = ((point.1 - 216) / 70) as usize;
            if row < 4 {
                match row * 2 + col {
                    0 => {
                        if files.is_dir[index] {
                            files.action = None;
                            files.enter(index);
                        } else {
                            files.open_preview(index);
                            if files.binary {
                                files.set_status("Binary file - cannot edit");
                                files.preview = None;
                                files.action = None;
                            } else {
                                files.open_editor(index);
                            }
                        }
                    }
                    1 => files.start_prompt(1, Some(index)),
                    2 => files.clipboard_set(index, false),
                    3 => files.clipboard_set(index, true),
                    4 => {
                        if files.confirm_delete {
                            files.delete_entry(index);
                        } else {
                            files.confirm_delete = true;
                        }
                    }
                    5 => {
                        files.props = Some(index);
                        files.action = None;
                    }
                    6 => {
                        if files.is_executable(index) {
                            files.run_entry(index);
                        } else {
                            files.set_status("Not an executable (ELF or /disk/bin)");
                        }
                    }
                    _ => files.export_entry(index),
                }
                return;
            }
        }
        if point.1 >= 576 && point.1 <= 620 {
            files.close_overlays();
            return;
        }
        files.close_overlays();
        return;
    }
    if let Some(index) = files.preview {
        if point.1 >= 676 && point.1 <= 716 {
            if point.0 >= 20 && point.0 <= 120 {
                if !files.binary {
                    files.open_editor(index);
                }
            } else if point.0 >= 130 && point.0 <= 230 {
                if files.is_executable(index) {
                    files.run_entry(index);
                } else {
                    files.set_status("Not an executable");
                }
            } else if point.0 >= 240 && point.0 <= 340 {
                files.export_entry(index);
                files.preview = None;
            } else if point.0 >= 350 && point.0 <= 456 {
                files.props = Some(index);
                files.preview = None;
            }
            return;
        }
        files.preview = None;
        return;
    }
    if point.1 >= 150 && point.1 <= 186 {
        if point.0 >= 20 && point.0 <= 90 {
            files.go_root();
        } else if point.0 >= 98 && point.0 <= 174 {
            files.start_prompt(2, None);
        } else if point.0 >= 182 && point.0 <= 258 {
            files.start_prompt(3, None);
        } else if point.0 >= 266 && point.0 <= 354 {
            if files.clip_len > 0 {
                files.paste();
            } else {
                let dir = slice_str(&files.dir[..files.dir_len]);
                if k::import_file(dir) {
                    files.set_status("Import started - refresh after");
                } else {
                    files.set_status("Import unavailable");
                }
            }
        } else if point.0 >= 362 && point.0 <= 456 {
            files.refresh();
        }
        return;
    }
    if point.1 >= 708 && point.1 <= 726 && files.clip_len > 0 {
        files.clip_len = 0;
        files.clip_name_len = 0;
        files.set_status("Clipboard cleared");
        return;
    }
    if point.1 >= 728 && point.1 <= 764 {
        if point.0 >= 20 && point.0 <= 140 {
            files.scroll = files.scroll.saturating_sub(1);
        } else if point.0 >= 340 && point.0 <= 460 {
            if files.scroll + 17 < files.count {
                files.scroll += 1;
            }
        }
        return;
    }
    if let Some(row) = files_row_at(point) {
        let index = files.scroll + row;
        if index < files.count {
            files.enter(index);
        }
    }
}

fn files_long_press(apps: &mut Apps, point: (i64, i64)) -> bool {
    let files = &mut apps.files;
    if files.preview.is_some()
        || files.editor.is_some()
        || files.prompt != 0
        || files.action.is_some()
        || files.props.is_some()
        || files.run_show
    {
        return false;
    }
    if let Some(row) = files_row_at(point) {
        let index = files.scroll + row;
        if index < files.count && files.names[index][..files.name_len[index] as usize] != b".."[..] {
            files.action = Some(index);
            files.confirm_delete = false;
            return true;
        }
    }
    false
}

fn files_key(apps: &mut Apps, code: u16, shift: bool) -> bool {
    let files = &mut apps.files;
    if files.prompt != 0 {
        return match key_event(code, shift) {
            Some((0, true)) => {
                files.prompt_len = files.prompt_len.saturating_sub(1);
                true
            }
            Some((b'\n', _)) => {
                files.prompt_confirm();
                true
            }
            Some((byte, _)) => {
                files.prompt_push(byte);
                true
            }
            None => false,
        };
    }
    if files.editor.is_some() {
        return match key_event(code, shift) {
            Some((0, true)) => {
                files.edit_len = files.edit_len.saturating_sub(1);
                true
            }
            Some((byte, _)) => {
                if files.edit_len < files.edit.len() {
                    files.edit[files.edit_len] = byte;
                    files.edit_len += 1;
                }
                true
            }
            None => false,
        };
    }
    false
}

fn weather_tap(apps: &mut Apps, point: (i64, i64)) {
    if point.1 >= 104 && point.1 <= 144 && point.0 >= 300 && point.0 <= 456 {
        apps.weather.fahrenheit = !apps.weather.fahrenheit;
        return;
    }
    if point.1 >= 368 && point.1 <= 408 && point.0 >= 20 && point.0 <= 160 {
        apps.weather.refresh();
    }
}

pub fn draw(screen: &Screen, apps: &mut Apps, app: AppKind, clock: [u64; 7], osk: bool) {
    match app {
        AppKind::Music => draw_music(screen, &apps.music),
        AppKind::Notes => draw_notes(screen, &apps.notes, osk),
        AppKind::Maps => draw_maps(screen, &apps.maps),
        AppKind::Photos => draw_photos(screen, &apps.photos),
        AppKind::Files => draw_files(screen, &apps.files, osk),
        AppKind::Weather => draw_weather(screen, &apps.weather, clock),
        AppKind::Terminal => draw_terminal(screen, &apps.terminal, osk),
        AppKind::Browser => draw_browser(screen, &mut apps.browser, osk),
        AppKind::Apk => draw_apk(screen, &apps.apk),
        AppKind::Dvm => draw_dvm(screen, &apps.dvm),
        AppKind::Market => draw_market(screen, &apps.market),
        AppKind::Monitor => draw_monitor(screen, &mut apps.monitor),
        AppKind::Plugins => draw_plugins(screen, &apps.plugins),
        _ => {}
    }
}

fn draw_music(screen: &Screen, music: &Music) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    button(screen, 236, 104, 110, 40, "IMPORT", SURFACE_HI(), TEXT());
    button(screen, 352, 104, 104, 40, "Rescan", SURFACE_HI(), MUTED());
    screen.gradient(40, 160, 160, 120, 0x0030_2028, RED_DIM());
    screen.circle(120, 220, 46, 0x0010_0c10);
    screen.ring(120, 220, 46, 2, RED());
    let (pos, dur, playing) = match k::audio_query() {
        Some(q) => (q[0], q[1], q[2] != 0),
        None => (music.pos_ms, music.dur_ms, music.playing),
    };
    if music.count > 0 {
        let name = slice_str(&music.names[music.track][..music.name_len[music.track] as usize]);
        let shown = name.len().min(28);
        screen.text(40, 300, &name[..shown], 2, TEXT());
    } else {
        screen.text(40, 300, "No tracks", 2, MUTED());
    }
    if music.status_len > 0 {
        screen.text(40, 336, slice_str(&music.status[..music.status_len]), 1, RED());
    } else {
        screen.text(40, 336, "/disk/music", 1, MUTED());
    }
    screen.round_rect(40, 380, width - 80, 8, 4, OUTLINE());
    let progress = if dur > 0 {
        ((pos.min(dur) * 1000 / dur) as i64).clamp(0, 1000)
    } else {
        0
    };
    screen.round_rect(40, 380, (width - 80) * progress / 1000, 8, 4, RED());
    let mut time = [0u8; 12];
    let left = format_ms(pos, &mut time);
    screen.text(40, 400, left, 1, MUTED());
    let mut time2 = [0u8; 12];
    let right = format_ms(dur, &mut time2);
    screen.text(width - 40 - Screen::text_width(right, 1), 400, right, 1, MUTED());
    screen.circle(140, 470, 28, SURFACE_HI());
    screen.line(148, 458, 148, 482, 3, TEXT());
    screen.line(148, 458, 132, 470, 3, TEXT());
    screen.line(148, 482, 132, 470, 3, TEXT());
    draw_play_button(screen, 240, 470, playing);
    screen.circle(340, 470, 28, SURFACE_HI());
    screen.line(332, 458, 332, 482, 3, TEXT());
    screen.line(332, 458, 348, 470, 3, TEXT());
    screen.line(332, 482, 348, 470, 3, TEXT());
    screen.text(24, 524, "TRACKS", 1, MUTED());
    button(screen, 368, 516, 40, 28, "up", SURFACE_HI(), MUTED());
    button(screen, 416, 516, 40, 28, "dn", SURFACE_HI(), MUTED());
    if music.count == 0 {
        screen.text_centered(width / 2, 580, "Import music to begin", 2, MUTED());
        return;
    }
    for row in 0..4usize {
        let index = music.scroll + row;
        if index >= music.count {
            break;
        }
        let y = 548 + row as i64 * 52;
        let active = index == music.track;
        screen.round_rect(20, y, width - 40, 46, 18, if active { SURFACE_HI() } else { SURFACE() });
        let name = slice_str(&music.names[index][..music.name_len[index] as usize]);
        let shown = name.len().min(34);
        screen.text(40, y + 15, &name[..shown], 2, if active { TEXT() } else { MUTED() });
        if active {
            screen.circle(width - 46, y + 23, 6, RED());
        }
    }
}

fn draw_play_button(screen: &Screen, cx: i64, cy: i64, playing: bool) {
    screen.circle(cx, cy, 38, RED());
    if playing {
        screen.fill(cx - 10, cy - 13, 6, 26, TEXT());
        screen.fill(cx + 4, cy - 13, 6, 26, TEXT());
    } else {
        screen.line(cx - 8, cy - 14, cx - 8, cy + 14, 4, TEXT());
        screen.line(cx - 8, cy - 14, cx + 14, cy, 4, TEXT());
        screen.line(cx - 8, cy + 14, cx + 14, cy, 4, TEXT());
    }
}

fn draw_notes(screen: &Screen, notes: &Notes, osk: bool) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    let Some(index) = notes.editing else {
        screen.text(24, 110, "NOTES", 2, TEXT());
        if notes.status_len > 0 {
            let shown = notes.status_len.min(28);
            screen.text(130, 118, slice_str(&notes.status[..shown]), 1, GREEN());
        }
        button(screen, 356, 104, 100, 44, "NEW", RED(), TEXT());
        if notes.count == 0 {
            screen.text_centered(width / 2, 380, "No notes yet", 2, MUTED());
            screen.text_centered(width / 2, 416, "Tap NEW to create one", 1, MUTED());
            return;
        }
        for row in 0..10usize {
            let i = notes.scroll + row;
            if i >= notes.count {
                break;
            }
            let y = 160 + row as i64 * 56;
            screen.round_rect(20, y, width - 40, 50, 18, SURFACE());
            let title = slice_str(&notes.titles[i][..notes.title_len[i] as usize]);
            let shown = title.len().min(34);
            screen.text(40, y + 17, &title[..shown], 2, TEXT());
        }
        button(screen, 20, 728, 120, 36, "UP", SURFACE_HI(), MUTED());
        button(screen, 340, 728, 120, 36, "DOWN", SURFACE_HI(), MUTED());
        let mut buf = [0u8; 20];
        let count = unsigned(notes.count as u64, &mut buf);
        screen.text_centered(width / 2, 738, count, 1, MUTED());
        return;
    };
    let title = slice_str(&notes.titles[index][..notes.title_len[index] as usize]);
    let shown = title.len().min(16);
    screen.text(24, 110, &title[..shown], 2, TEXT());
    button(screen, 236, 104, 104, 44, "SAVE", RED(), TEXT());
    button(screen, 348, 104, 108, 44, "DEL", SURFACE_HI(), RED());
    let panel_height = if osk { 260 } else { 454 };
    let text_limit = if osk { 386 } else { 580 };
    screen.round_rect(20, 160, width - 40, panel_height, 26, SURFACE());
    let mut x = 40i64;
    let mut y = 180i64;
    let mut line_length = 0;
    for &byte in &notes.text[..notes.len] {
        if byte == b'\n' {
            x = 40;
            y += 15;
            line_length = 0;
            continue;
        }
        if line_length == 50 {
            x = 40;
            y += 15;
            line_length = 0;
        }
        if y > text_limit {
            break;
        }
        let ch = [byte];
        screen.text(x, y, slice_str(&ch), 1, TEXT());
        x += 8;
        line_length += 1;
    }
    let status_y = if osk { 430 } else { 626 };
    if notes.status_len > 0 {
        screen.text(24, status_y, slice_str(&notes.status[..notes.status_len]), 1, GREEN());
    } else {
        screen.text(24, status_y, "SAVE writes this note to /disk/notes", 1, MUTED());
    }
    if !osk {
        screen.text(24, 654, "Back arrow saves and returns to the list", 1, MUTED());
    }
}

fn draw_maps(screen: &Screen, maps: &Maps) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    screen.round_rect(20, 108, width - 40, 512, 28, 0x0018_1c22);
    screen.fill(28, 116, width - 56, 496, 0x0014_2028);
    for index in 0..7 {
        let y = 150 + index * 62;
        screen.line(20, y, width - 20, y + 30, 2, 0x0028_3038);
    }
    for index in 0..5 {
        let x = 60 + index * 90;
        screen.line(x, 108, x - 30, 620, 3, 0x0024_2c34);
    }
    screen.round_rect(40, 400, 130, 100, 18, 0x0020_3830);
    screen.text(56, 420, "Park", 1, GREEN());
    screen.line(250, 108, 210, 620, 14, 0x0020_3850);
    screen.line(250, 108, 210, 620, 8, 0x0030_5878);
    screen.text(70, 250, "Kimi City", 2, TEXT());
    screen.text(70, 278, "grid 7 / 12", 1, MUTED());
    if let Some((px, py)) = maps.pin {
        screen.circle(px, py, 16, RED());
        screen.circle(px, py, 6, TEXT());
        screen.fill(px - 3, py + 12, 6, 10, RED());
    } else {
        screen.text_centered(width / 2, 350, "Tap the map to drop a pin", 2, TEXT());
    }
    screen.round_rect(20, 636, width - 40, 104, 26, SURFACE());
    if maps.has_fix {
        screen.text(40, 650, "Current location", 2, TEXT());
        let mut line = [0u8; 48];
        let mut used = 0;
        let mut coord = [0u8; 24];
        push_str(&mut line, &mut used, format_coord(maps.lat, &mut coord).as_bytes());
        push_str(&mut line, &mut used, b", ");
        push_str(&mut line, &mut used, format_coord(maps.lon, &mut coord).as_bytes());
        screen.text(40, 684, slice_str(&line[..used]), 1, MUTED());
        button(screen, 296, 656, 144, 44, "Open map", RED(), TEXT());
    } else {
        screen.text(40, 650, "Location unavailable", 2, MUTED());
        screen.text(40, 684, "Enable location to open the live map", 1, MUTED());
    }
    if maps.status_len > 0 {
        screen.text(24, 752, slice_str(&maps.status[..maps.status_len]), 1, MUTED());
    }
}

fn draw_photos(screen: &Screen, photos: &Photos) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    if let Some(index) = photos.selected {
        draw_photo(screen, index, 24, 118, width - 48, 470);
        screen.text(24, 608, "Photo", 2, TEXT());
        let mut buf = [0u8; 20];
        let value = unsigned((index + 1) as u64, &mut buf);
        screen.text(88, 608, value, 2, MUTED());
        screen.text(24, 646, "Tap anywhere to close", 1, MUTED());
        return;
    }
    screen.text(24, 102, "GALLERY", 1, MUTED());
    for index in 0..6usize {
        let column = index as i64 % 2;
        let row = index as i64 / 2;
        let x = 24 + column * 224;
        let y = 120 + row * 156;
        draw_photo(screen, index, x, y, 208, 144);
        screen.text(x + 6, y + 148 - 18, "img", 1, TEXT());
    }
    screen.text(24, 648, "Tap a photo to open it", 1, MUTED());
}

fn draw_photo(screen: &Screen, index: usize, x: i64, y: i64, width: i64, height: i64) {
    match index % 6 {
        0 => {
            screen.gradient(x, y, width, height, 0x00f0_8040, 0x0040_1020);
            screen.circle(x + width * 3 / 4, y + height / 3, height / 6, 0x00ff_e080);
            screen.fill(x, y + height - 12, width, 12, 0x0020_1018);
        }
        1 => {
            screen.gradient(x, y, width, height, 0x0030_6080, 0x0010_2030);
            screen.fill(x, y + height / 2, width, 2, 0x0080_c0d0);
            for line in 0..4 {
                let yy = y + height / 2 + 8 + line * 12;
                screen.fill(x, yy, width, 1, 0x0040_7080);
            }
        }
        2 => {
            screen.gradient(x, y, width, height, 0x0010_1420, 0x0030_2030);
            for building in 0..5 {
                let bx = x + 12 + building * (width - 24) / 5;
                let bh = 20 + (building * 37 % 60);
                screen.fill(bx, y + height - bh - 20, (width - 24) / 7, bh, 0x0020_2838);
                for row in 0..3 {
                    screen.fill(bx + 3, y + height - bh - 10 + row * 8, 3, 3, 0x00d0_c060);
                }
            }
        }
        3 => {
            screen.gradient(x, y, width, height, 0x0040_6070, 0x0010_1818);
            screen.circle(x + width / 3, y + height / 2, height / 3, 0x0028_3830);
            screen.circle(x + width * 2 / 3, y + height / 2, height / 2, 0x0018_2820);
            screen.fill(x, y + height - 18, width, 18, 0x0010_1414);
        }
        4 => {
            screen.gradient(x, y, width, height, 0x0020_1828, 0x0060_2040);
            for ring in 0..4 {
                screen.ring(x + width / 2, y + height / 2, 18 + ring * 12, 2, RED());
            }
        }
        _ => {
            screen.gradient(x, y, width, height, 0x0018_2820, 0x0040_6040);
            for step in 0..8 {
                screen.line(x + step * width / 8, y, x, y + step * height / 8, 1, 0x0050_8060);
            }
        }
    }
}

fn draw_files_editor(screen: &Screen, files: &Files, osk: bool) {
    let width = screen.width();
    let index = files.editor.unwrap_or(0);
    let title = slice_str(&files.names[index][..files.name_len[index] as usize]);
    let shown = title.len().min(24);
    screen.text(24, 110, &title[..shown], 2, TEXT());
    button(screen, 236, 104, 104, 44, "SAVE", RED(), TEXT());
    button(screen, 348, 104, 108, 44, "CLOSE", SURFACE_HI(), TEXT());
    let panel_height = if osk { 260 } else { 454 };
    let text_limit = if osk { 386 } else { 580 };
    screen.round_rect(20, 160, width - 40, panel_height, 26, SURFACE());
    let mut x = 40i64;
    let mut y = 180i64;
    let mut line_length = 0;
    for &byte in &files.edit[..files.edit_len] {
        if byte == b'\n' {
            x = 40;
            y += 15;
            line_length = 0;
            continue;
        }
        if line_length == 50 {
            x = 40;
            y += 15;
            line_length = 0;
        }
        if y > text_limit {
            break;
        }
        screen.text(x, y, slice_str(&[byte]), 1, TEXT());
        x += 8;
        line_length += 1;
    }
    let status_y = if osk { 430 } else { 626 };
    if files.status_len > 0 {
        screen.text(24, status_y, slice_str(&files.status[..files.status_len]), 1, GREEN());
    } else {
        screen.text(24, status_y, "SAVE writes back to this file", 1, MUTED());
    }
    if !osk {
        let mut buf = [0u8; 20];
        let size = unsigned(files.edit_len as u64, &mut buf);
        let mut line = [0u8; 48];
        let mut used = 0;
        push_str(&mut line, &mut used, size.as_bytes());
        push_str(&mut line, &mut used, b" bytes (2048 max) - back saves & closes");
        screen.text(24, 654, slice_str(&line[..used]), 1, MUTED());
    }
}

fn draw_files_prompt(screen: &Screen, files: &Files) {
    let width = screen.width();
    screen.fill_blend(0, 93, width, screen.height() - 93, BLACK(), 160);
    screen.round_rect(40, 300, width - 80, 180, 26, SURFACE_HI());
    let label = match files.prompt { 1 => "RENAME", 2 => "NEW FILE", _ => "NEW FOLDER" };
    screen.text(60, 316, label, 2, TEXT());
    screen.round_rect(60, 352, width - 120, 44, 12, SURFACE());
    let shown = files.prompt_len.min(40);
    if shown == 0 {
        screen.text(72, 366, "name", 1, MUTED());
    } else {
        screen.text(72, 366, slice_str(&files.prompt_buf[..shown]), 2, TEXT());
    }
    button(screen, 240, 420, 96, 44, "OK", RED(), TEXT());
    button(screen, 348, 420, 96, 44, "Cancel", SURFACE_MAX(), MUTED());
}

fn draw_files_action(screen: &Screen, files: &Files, index: usize) {
    let width = screen.width();
    screen.fill_blend(0, 93, width, screen.height() - 93, BLACK(), 160);
    screen.round_rect(40, 196, width - 80, 448, 26, SURFACE_HI());
    let name = slice_str(&files.names[index][..files.name_len[index] as usize]);
    let shown = name.len().min(30);
    screen.text(56, 204, &name[..shown], 1, MUTED());
    let labels = [
        if files.is_dir[index] { "Open" } else { "Edit" },
        "Rename",
        "Copy",
        "Move",
        if files.confirm_delete { "SURE?" } else { "Delete" },
        "Properties",
        "Run",
        "Export",
    ];
    for (i, label) in labels.iter().enumerate() {
        let col = (i % 2) as i64;
        let row = (i / 2) as i64;
        let x = 56 + col * 188;
        let y = 216 + row * 70;
        let danger = i == 4;
        let bg = if danger { if files.confirm_delete { RED() } else { RED_DIM() } } else { SURFACE_MAX() };
        button(screen, x, y, 180, 56, label, bg, TEXT());
    }
    button(screen, 150, 576, 180, 44, "Cancel", SURFACE(), MUTED());
}

fn draw_files_props(screen: &Screen, files: &Files, index: usize) {
    let width = screen.width();
    screen.fill_blend(0, 93, width, screen.height() - 93, BLACK(), 160);
    screen.round_rect(40, 220, width - 80, 340, 26, SURFACE_HI());
    screen.text(60, 236, "PROPERTIES", 2, TEXT());
    let name = slice_str(&files.names[index][..files.name_len[index] as usize]);
    screen.text(60, 280, "Name", 1, MUTED());
    let shown = name.len().min(34);
    screen.text(140, 280, &name[..shown], 1, TEXT());
    screen.text(60, 310, "Path", 1, MUTED());
    let path = slice_str(&files.paths[index][..files.path_len[index] as usize]);
    let plen = path.len();
    screen.text(140, 310, &path[..plen.min(34)], 1, TEXT());
    if plen > 34 {
        screen.text(140, 328, &path[34..plen.min(68)], 1, TEXT());
    }
    screen.text(60, 358, "Kind", 1, MUTED());
    let kind = if files.is_dir[index] {
        "Directory"
    } else if files.is_executable(index) {
        "Executable"
    } else {
        "File"
    };
    screen.text(140, 358, kind, 1, TEXT());
    screen.text(60, 388, "Size", 1, MUTED());
    let mut buf = [0u8; 20];
    let size = unsigned(files.sizes[index], &mut buf);
    let mut line = [0u8; 32];
    let mut used = 0;
    push_str(&mut line, &mut used, size.as_bytes());
    push_str(&mut line, &mut used, b" bytes");
    screen.text(140, 388, slice_str(&line[..used]), 1, TEXT());
    if let Some((kind_id, _)) = k::stat(path) {
        screen.text(60, 418, "Storage", 1, MUTED());
        screen.text(140, 418, if kind_id == 1 { "Disk (KimiFS)" } else { "RamFS" }, 1, TEXT());
    }
    screen.text(60, 520, "Tap anywhere to close", 1, MUTED());
}

fn draw_files(screen: &Screen, files: &Files, osk: bool) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    if files.editor.is_some() {
        draw_files_editor(screen, files, osk);
        return;
    }
    if let Some(index) = files.preview {
        screen.round_rect(20, 104, width - 40, 560, 26, SURFACE());
        let title = slice_str(&files.names[index][..files.name_len[index] as usize]);
        screen.text(40, 120, title, 2, TEXT());
        if files.binary {
            screen.text(40, 160, "Binary file", 1, MUTED());
        } else {
            let mut x = 40i64;
            let mut y = 160i64;
            let mut column = 0;
            for &byte in &files.body[..files.body_len] {
                if byte == b'\n' || column == 54 {
                    x = 40;
                    y += 13;
                    column = 0;
                    if byte == b'\n' {
                        continue;
                    }
                }
                if y > 630 {
                    break;
                }
                screen.text(x, y, slice_str(&[byte]), 1, TEXT());
                x += 8;
                column += 1;
            }
        }
        screen.text(40, 642, "Tap body to close - long-press list for actions", 1, MUTED());
        button(screen, 20, 676, 100, 40, "EDIT", if files.binary { SURFACE() } else { SURFACE_HI() }, if files.binary { MUTED() } else { TEXT() });
        button(screen, 130, 676, 100, 40, "RUN", SURFACE_HI(), TEXT());
        button(screen, 240, 676, 100, 40, "EXPORT", SURFACE_HI(), TEXT());
        button(screen, 350, 676, 106, 40, "PROPS", SURFACE_HI(), TEXT());
        if files.status_len > 0 {
            screen.text(24, 730, slice_str(&files.status[..files.status_len]), 1, RED());
        }
        if let Some(index) = files.action {
            draw_files_action(screen, files, index);
        }
        return;
    }
    screen.round_rect(20, 100, width - 40, 42, 21, SURFACE());
    let dir = slice_str(&files.dir[..files.dir_len]);
    let start = files.dir_len.saturating_sub(50);
    screen.text(36, 112, &dir[start..], 1, TEXT());
    button(screen, 20, 150, 70, 36, "Root", SURFACE_HI(), TEXT());
    button(screen, 98, 150, 76, 36, "+File", SURFACE_HI(), TEXT());
    button(screen, 182, 150, 76, 36, "+Dir", SURFACE_HI(), TEXT());
    if files.clip_len > 0 {
        button(screen, 266, 150, 88, 36, "Paste", RED(), TEXT());
    } else {
        button(screen, 266, 150, 88, 36, "Import", SURFACE_HI(), TEXT());
    }
    button(screen, 362, 150, 94, 36, "Refresh", SURFACE_HI(), MUTED());
    for row in 0..17usize {
        let index = files.scroll + row;
        if index >= files.count {
            break;
        }
        let y = 196 + row as i64 * 30;
        screen.round_rect(20, y, width - 40, 28, 14, if row % 2 == 0 { SURFACE() } else { BLACK() });
        screen.round_rect(34, y + 7, 14, 14, 3, if files.is_dir[index] { RED_DIM() } else { MUTED() });
        let label = slice_str(&files.names[index][..files.name_len[index] as usize]);
        let shown = label.len().min(36);
        screen.text(60, y + 10, &label[..shown], 1, TEXT());
        if files.is_dir[index] {
            screen.text(width - 60, y + 10, "DIR", 1, MUTED());
        } else {
            let mut buf = [0u8; 20];
            let size = unsigned(files.sizes[index], &mut buf);
            screen.text(width - 28 - Screen::text_width(size, 1), y + 10, size, 1, MUTED());
        }
    }
    if files.count == 0 {
        screen.text_centered(width / 2, 400, "Empty directory", 2, MUTED());
    }
    button(screen, 20, 728, 120, 36, "UP", SURFACE_HI(), MUTED());
    button(screen, 340, 728, 120, 36, "DOWN", SURFACE_HI(), MUTED());
    let mut line = [0u8; 48];
    let mut used = 0;
    let mut buf = [0u8; 20];
    push_str(&mut line, &mut used, unsigned(files.count as u64, &mut buf).as_bytes());
    push_str(&mut line, &mut used, b" items");
    screen.text_centered(width / 2, 738, slice_str(&line[..used]), 1, MUTED());
    if files.clip_len > 0 {
        let mut chip = [0u8; 64];
        let mut cn = 0;
        push_str(&mut chip, &mut cn, if files.clip_cut { b"MOVE: " } else { b"COPY: " });
        let nl = files.clip_name_len.min(36);
        push_str(&mut chip, &mut cn, &files.clip_name[..nl]);
        push_str(&mut chip, &mut cn, b" (tap to clear)");
        screen.text(24, 710, slice_str(&chip[..cn]), 1, RED());
    } else if files.status_len > 0 {
        screen.text(24, 712, slice_str(&files.status[..files.status_len]), 1, RED());
    } else {
        screen.text(24, 712, "Long-press an entry for actions", 1, MUTED());
    }
    if files.run_show {
        screen.fill_blend(0, 93, width, screen.height() - 93, BLACK(), 160);
        screen.round_rect(40, 300, width - 80, 160, 26, SURFACE_HI());
        screen.text(60, 316, "EXECUTE", 2, TEXT());
        screen.text(60, 360, slice_str(&files.run_msg[..files.run_len]), 1, TEXT());
        screen.text(60, 420, "Tap to close", 1, MUTED());
    } else if let Some(index) = files.props {
        draw_files_props(screen, files, index);
    } else if let Some(index) = files.action {
        draw_files_action(screen, files, index);
    }
    if files.prompt != 0 {
        draw_files_prompt(screen, files);
    }
}

fn weather_temp_text<'a>(weather: &Weather, x10: i64, buf: &'a mut [u8; 20]) -> &'a str {
    let value = if weather.fahrenheit { x10 * 9 / 5 + 320 } else { x10 };
    signed(round_tenths(value), buf)
}

fn draw_weather(screen: &Screen, weather: &Weather, clock: [u64; 7]) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    screen.round_rect(20, 104, width - 40, 240, 30, SURFACE());
    button(screen, 304, 104, 152, 40, if weather.fahrenheit { "USE C" } else { "USE F" }, SURFACE_HI(), MUTED());
    let unit = if weather.fahrenheit { "F" } else { "C" };
    if weather.has_data {
        let mut buf = [0u8; 20];
        let text = weather_temp_text(weather, weather.temp, &mut buf);
        screen.text(40, 132, text, 5, TEXT());
        screen.text(40 + Screen::text_width(text, 5) + 8, 140, unit, 2, MUTED());
        let cond = if weather.cond_len > 0 {
            slice_str(&weather.cond[..weather.cond_len])
        } else {
            weather_text(weather.code)
        };
        screen.text(40, 220, cond, 2, TEXT());
        if weather.feels != FEELS_NONE {
            let mut feels = [0u8; 20];
            let value = weather_temp_text(weather, weather.feels, &mut feels);
            let mut line = [0u8; 48];
            let mut used = 0;
            push_str(&mut line, &mut used, b"Feels like ");
            push_str(&mut line, &mut used, value.as_bytes());
            push_str(&mut line, &mut used, b" ");
            push_str(&mut line, &mut used, unit.as_bytes());
            screen.text(40, 252, slice_str(&line[..used]), 1, MUTED());
        }
        let mut line = [0u8; 64];
        let mut used = 0;
        if weather.humidity >= 0 {
            let mut buf = [0u8; 20];
            push_str(&mut line, &mut used, b"Humidity ");
            push_str(&mut line, &mut used, unsigned(weather.humidity as u64, &mut buf).as_bytes());
            push_str(&mut line, &mut used, b"%");
        }
        if weather.wind >= 0 {
            if used > 0 {
                push_str(&mut line, &mut used, b"   ");
            }
            let mut wind = [0u8; 20];
            push_str(&mut line, &mut used, b"Wind ");
            push_str(&mut line, &mut used, format_tenths(weather.wind, &mut wind).as_bytes());
            push_str(&mut line, &mut used, b" km/h");
        }
        if used > 0 {
            screen.text(40, 276, slice_str(&line[..used]), 1, MUTED());
        }
        let provider = match weather.provider {
            1 => "HKO - Hong Kong",
            2 => "Open-Meteo",
            _ => "",
        };
        screen.text(40, 306, provider, 1, OUTLINE());
        screen.circle(392, 248, 34, 0x00f0_c050);
        for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0), (1, 1), (-1, 1), (1, -1), (-1, -1)] {
            screen.line(392 + dx * 42, 248 + dy * 42, 392 + dx * 56, 248 + dy * 56, 2, 0x00f0_c050);
        }
    } else if weather.has_fix {
        screen.text(40, 150, "No weather data", 2, MUTED());
        screen.text(40, 190, "Tap Refresh to try again", 1, MUTED());
    } else {
        screen.text(40, 150, "Location unavailable", 2, MUTED());
        screen.text(40, 190, "Waiting for a location fix", 1, MUTED());
    }
    button(screen, 20, 368, 140, 40, "Refresh", RED(), TEXT());
    screen.text(24, 436, "CURRENT LOCATION", 1, MUTED());
    if weather.has_fix {
        let mut line = [0u8; 64];
        let mut used = 0;
        let mut coord = [0u8; 24];
        push_str(&mut line, &mut used, b"Lat ");
        push_str(&mut line, &mut used, format_coord(weather.lat, &mut coord).as_bytes());
        push_str(&mut line, &mut used, b"  Lon ");
        push_str(&mut line, &mut used, format_coord(weather.lon, &mut coord).as_bytes());
        screen.text(24, 464, slice_str(&line[..used]), 1, TEXT());
        let mut buf = [0u8; 20];
        let acc = unsigned(weather.accuracy, &mut buf);
        let mut line2 = [0u8; 48];
        let mut used2 = 0;
        push_str(&mut line2, &mut used2, b"Accuracy ");
        push_str(&mut line2, &mut used2, acc.as_bytes());
        push_str(&mut line2, &mut used2, b" m");
        screen.text(24, 490, slice_str(&line2[..used2]), 1, MUTED());
    } else {
        screen.text(24, 464, "No fix", 1, MUTED());
    }
    if weather.provider == 1 && weather.forecast_count > 0 {
        screen.text(24, 524, "9-DAY FORECAST", 1, MUTED());
        for index in 0..4usize {
            if index >= weather.forecast_count {
                break;
            }
            let day = &weather.forecast[index];
            let x = 20 + index as i64 * 110;
            let y = 548i64;
            screen.round_rect(x, y, 100, 150, 18, SURFACE());
            screen.text(x + 12, y + 14, slice_str(&day.week), 1, TEXT());
            let cond = hko_icon_text(day.icon);
            let shown = cond.len().min(10);
            screen.text(x + 12, y + 40, &cond[..shown], 1, MUTED());
            let mut max_buf = [0u8; 20];
            let max = weather_temp_text(weather, day.max, &mut max_buf);
            let mut min_buf = [0u8; 20];
            let min = weather_temp_text(weather, day.min, &mut min_buf);
            let mut line = [0u8; 24];
            let mut used = 0;
            push_str(&mut line, &mut used, max.as_bytes());
            push_str(&mut line, &mut used, b"/");
            push_str(&mut line, &mut used, min.as_bytes());
            screen.text(x + 12, y + 72, slice_str(&line[..used]), 2, TEXT());
            screen.text(x + 12, y + 108, unit, 1, MUTED());
        }
    }
    let _ = clock;
    screen.text(24, 730, slice_str(&weather.status[..weather.status_len]), 1, MUTED());
}

fn draw_terminal(screen: &Screen, terminal: &Terminal, osk: bool) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    let max_lines = if osk { 24 } else { TERM_LINES };
    let separator_y = if osk { 416 } else { 684 };
    let input_y = if osk { 430 } else { 700 };
    let start = terminal.count.saturating_sub(max_lines);
    let mut y = 108i64;
    for index in start..terminal.count {
        let length = terminal.line_len[index] as usize;
        screen.text_mono(20, y, slice_str(&terminal.lines[index][..length]), 1, TEXT());
        y += 13;
    }
    screen.fill(20, separator_y, width - 40, 1, OUTLINE());
    screen.text_mono(20, input_y, "$", 2, RED());
    let shown = terminal.input_len.min(52);
    screen.text_mono(44, input_y, slice_str(&terminal.input[..shown]), 2, TEXT());
    if terminal.input_len == 0 {
        screen.text_mono(44, input_y, "type a command", 2, 0x0050_505a);
    }
    if !osk {
        screen.text_mono(20, 744, "help  ls  cat  date  df  mem  ps  uptime  clear", 1, MUTED());
    }
}

fn browser_tap(apps: &mut Apps, point: (i64, i64), osk: bool) {
    let width = 480i64;
    if point.0 >= width - 84 && point.0 <= width - 20 && point.1 >= 104 && point.1 <= 148 {
        browser_go(apps);
        return;
    }
    let browser = &mut apps.browser;
    if point.0 >= 20 && point.0 < width - 90 && point.1 >= 104 && point.1 <= 148 {
        if browser.web_ok && browser.page_url_len > 0 {
            let n = browser.page_url_len.min(browser.url.len());
            browser.url[..n].copy_from_slice(&browser.page_url[..n]);
            browser.url_len = n;
        }
        browser.editing = true;
        return;
    }
    if browser.web_ok && point.1 >= 152 && point.1 <= 188 {
        if point.0 >= 20 && point.0 <= 76 {
            if browser.can_back {
                k::web_back();
                browser.set_status("Back");
            }
            return;
        }
        if point.0 >= 84 && point.0 <= 140 {
            if browser.can_fwd {
                k::web_forward();
                browser.set_status("Forward");
            }
            return;
        }
        if point.0 >= 148 && point.0 <= 204 {
            k::web_reload();
            browser.set_status("Reloading...");
            return;
        }
    }
    if browser.web_ok && in_web_content(point) {
        browser.editing = false;
        return;
    }
    let scroll_y = if osk { 410 } else { 700 };
    if point.1 >= scroll_y && point.1 <= scroll_y + 40 {
        if point.0 >= width - 110 && point.0 <= width - 66 {
            browser.scroll = browser.scroll.saturating_sub(5);
            return;
        }
        if point.0 >= width - 60 && point.0 <= width - 16 {
            if browser.scroll + 34 < browser.line_count {
                browser.scroll += 5;
            }
            return;
        }
    }
    browser.editing = false;
}

fn browser_go(apps: &mut Apps) {
    let browser = &mut apps.browser;
    browser.editing = false;
    if browser.url_len == 0 {
        browser.set_status("Enter a host");
        return;
    }
    let mut full = [0u8; 160];
    let mut used = 0;
    let url = &browser.url[..browser.url_len];
    if !starts_ci(url, b"http://") && !starts_ci(url, b"https://") {
        push_str(&mut full, &mut used, b"https://");
    }
    push_str(&mut full, &mut used, url);
    if browser.web_ok {
        browser.loaded = true;
        if k::web_load(slice_str(&full[..used])) {
            browser.set_status("Loading...");
            return;
        }
        browser.web_ok = false;
    }
    browser_fetch_http(apps);
}

fn browser_fetch_http(apps: &mut Apps) {
    let browser = &mut apps.browser;
    browser.scroll = 0;
    browser.line_count = 0;
    browser.title_len = 0;
    browser.set_status("WebView unavailable - text fetch");

    let mut url_buf = [0u8; 128];
    let url_len = browser.url_len.min(128);
    url_buf[..url_len].copy_from_slice(&browser.url[..url_len]);
    let url = &url_buf[..url_len];
    let url = if starts_ci(url, b"http://") {
        &url[7..]
    } else if starts_ci(url, b"https://") {
        &url[8..]
    } else {
        url
    };
    let split = url.iter().position(|&b| b == b'/').unwrap_or(url.len());
    let host = &url[..split];
    let path = if split < url.len() { &url[split..] } else { &b"/"[..] };
    if host.is_empty() {
        browser.set_status("Enter a host");
        return;
    }
    let Ok(host_str) = core::str::from_utf8(host) else {
        browser.set_status("Bad host");
        return;
    };

    browser.set_status("Resolving...");
    k::net_status(5000);
    let Some(ip) = k::net_resolve(host_str) else {
        browser.set_status("DNS failed");
        return;
    };
    browser.set_status("Connecting...");
    let Some(slot) = k::net_tcp_connect(ip, 80, 5000) else {
        browser.set_status("Connect failed");
        return;
    };

    let mut request = [0u8; 320];
    let mut used = 0usize;
    for piece in [
        &b"GET "[..],
        path,
        b" HTTP/1.0\r\nHost: ",
        host,
        b"\r\nUser-Agent: KimiOS-Browser\r\nConnection: close\r\n\r\n",
    ] {
        if used + piece.len() > request.len() {
            break;
        }
        request[used..used + piece.len()].copy_from_slice(piece);
        used += piece.len();
    }
    k::net_tcp_send(slot, &request[..used]);
    browser.set_status("Downloading...");

    let mut body = [0u8; BROWSER_BODY];
    let mut body_len = 0usize;
    loop {
        let mut chunk = [0u8; 1024];
        let count = k::net_tcp_recv(slot, &mut chunk, 4000);
        if count == 0 {
            break;
        }
        let take = count.min(BROWSER_BODY - body_len);
        body[body_len..body_len + take].copy_from_slice(&chunk[..take]);
        body_len += take;
        if body_len >= BROWSER_BODY {
            break;
        }
    }
    k::net_tcp_close(slot);

    let start = header_end(&body[..body_len]);
    html_to_lines(&body[start..body_len], browser);
    if browser.line_count == 0 {
        browser.set_status("Empty page");
    } else {
        browser.set_status("Done");
    }
}

fn header_end(data: &[u8]) -> usize {
    let mut i = 0;
    while i + 3 < data.len() {
        if &data[i..i + 4] == b"\r\n\r\n" {
            return i + 4;
        }
        if &data[i..i + 2] == b"\n\n" {
            return i + 2;
        }
        i += 1;
    }
    0
}

fn eq_ci(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_ascii_lowercase() == *y)
}

fn starts_ci(data: &[u8], prefix: &[u8]) -> bool {
    data.len() >= prefix.len() && eq_ci(&data[..prefix.len()], prefix)
}

fn tag_base(tag: &[u8]) -> &[u8] {
    let mut t = tag;
    if t.first() == Some(&b'/') {
        t = &t[1..];
    }
    if t.last() == Some(&b'/') {
        t = &t[..t.len() - 1];
    }
    t
}

fn is_block_tag(tag: &[u8]) -> bool {
    let t = tag_base(tag);
    matches!(
        t,
        b"p" | b"div" | b"br" | b"li" | b"tr" | b"table" | b"ul" | b"ol"
            | b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6"
            | b"section" | b"article" | b"header" | b"footer" | b"blockquote"
            | b"pre" | b"hr" | b"head" | b"body" | b"html" | b"main" | b"nav"
    )
}

fn decode_entity(entity: &[u8]) -> Option<u8> {
    match entity {
        b"amp" => Some(b'&'),
        b"lt" => Some(b'<'),
        b"gt" => Some(b'>'),
        b"quot" => Some(b'"'),
        b"apos" => Some(b'\''),
        b"nbsp" => Some(b' '),
        _ => {
            if let Some(rest) = entity.strip_prefix(b"#") {
                let (digits, radix) = if let Some(hex) = rest.strip_prefix(b"x") {
                    (hex, 16)
                } else {
                    (rest, 10)
                };
                let mut value = 0u32;
                for &d in digits {
                    let digit = match d {
                        b'0'..=b'9' => (d - b'0') as u32,
                        b'a'..=b'f' => (d - b'a' + 10) as u32,
                        b'A'..=b'F' => (d - b'A' + 10) as u32,
                        _ => return None,
                    };
                    value = value * radix + digit;
                    if value > 0x10FFFF {
                        return None;
                    }
                }
                if value < 128 {
                    Some(value as u8)
                } else {
                    Some(b'?')
                }
            } else {
                None
            }
        }
    }
}

struct LineWriter<'a> {
    browser: &'a mut Browser,
    current: [u8; BROWSER_WIDTH],
    length: usize,
    pending_space: bool,
}

impl<'a> LineWriter<'a> {
    fn new(browser: &'a mut Browser) -> Self {
        Self { browser, current: [0u8; BROWSER_WIDTH], length: 0, pending_space: false }
    }

    fn flush(&mut self) {
        if self.length > 0 && self.browser.line_count < BROWSER_LINES {
            let index = self.browser.line_count;
            self.browser.lines[index][..self.length].copy_from_slice(&self.current[..self.length]);
            self.browser.line_len[index] = self.length as u8;
            self.browser.line_count += 1;
        }
        self.length = 0;
    }

    fn newline(&mut self) {
        if self.length > 0 {
            self.flush();
        } else if self.browser.line_count > 0
            && self.browser.line_len[self.browser.line_count - 1] > 0
            && self.browser.line_count < BROWSER_LINES
        {
            self.browser.line_len[self.browser.line_count] = 0;
            self.browser.line_count += 1;
        }
        self.pending_space = false;
    }

    fn put(&mut self, byte: u8) {
        if byte == b' ' || byte == b'\t' || byte == b'\n' || byte == b'\r' {
            self.pending_space = true;
            return;
        }
        if byte < 0x20 {
            return;
        }
        if self.pending_space && self.length > 0 {
            if self.length + 1 >= BROWSER_WIDTH {
                self.flush();
            } else {
                self.current[self.length] = b' ';
                self.length += 1;
            }
            self.pending_space = false;
        }
        if self.length >= BROWSER_WIDTH {
            self.flush();
        }
        if self.length < BROWSER_WIDTH {
            self.current[self.length] = byte;
            self.length += 1;
        }
    }
}

fn html_to_lines(html: &[u8], browser: &mut Browser) {
    let mut title_buf = [0u8; 64];
    let mut title_len = 0usize;
    let mut writer = LineWriter::new(browser);
    let mut i = 0usize;
    let n = html.len();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut in_comment = false;
    let mut in_title = false;
    let mut in_entity = false;
    let mut attr_started = false;
    let mut tag = [0u8; 24];
    let mut tag_len = 0usize;
    let mut entity = [0u8; 12];
    let mut entity_len = 0usize;

    while i < n {
        let c = html[i];
        if in_comment {
            if c == b'-' && starts_ci(&html[i..], b"-->") {
                in_comment = false;
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }
        if in_script {
            if c == b'<' && starts_ci(&html[i..], b"</script") {
                in_script = false;
                i += 8;
                continue;
            }
            i += 1;
            continue;
        }
        if in_style {
            if c == b'<' && starts_ci(&html[i..], b"</style") {
                in_style = false;
                i += 7;
                continue;
            }
            i += 1;
            continue;
        }
        if in_entity {
            if c == b';' {
                if let Some(decoded) = decode_entity(&entity[..entity_len]) {
                    if in_title {
                        if title_len < title_buf.len() {
                            title_buf[title_len] = decoded;
                            title_len += 1;
                        }
                    } else {
                        writer.put(decoded);
                    }
                }
                in_entity = false;
                entity_len = 0;
                i += 1;
                continue;
            }
            if entity_len < entity.len() && (c.is_ascii_alphanumeric() || c == b'#') {
                entity[entity_len] = c;
                entity_len += 1;
                i += 1;
                continue;
            }
            in_entity = false;
            entity_len = 0;
            writer.put(b'&');
        }
        if in_tag {
            if c == b'>' {
                in_tag = false;
                let t = &tag[..tag_len];
                if eq_ci(t, b"script") {
                    in_script = true;
                } else if eq_ci(t, b"style") {
                    in_style = true;
                } else if eq_ci(t, b"title") {
                    in_title = true;
                } else if eq_ci(t, b"/title") {
                    in_title = false;
                } else if is_block_tag(t) {
                    writer.newline();
                }
                tag_len = 0;
                i += 1;
                continue;
            }
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                attr_started = true;
            } else if !attr_started && tag_len < tag.len() {
                tag[tag_len] = c.to_ascii_lowercase();
                tag_len += 1;
            }
            i += 1;
            continue;
        }
        if c == b'<' {
            if starts_ci(&html[i..], b"<!--") {
                in_comment = true;
                i += 4;
                continue;
            }
            in_tag = true;
            tag_len = 0;
            attr_started = false;
            i += 1;
            continue;
        }
        if c == b'&' {
            in_entity = true;
            entity_len = 0;
            i += 1;
            continue;
        }
        if in_title {
            if title_len < title_buf.len() && c >= 0x20 {
                title_buf[title_len] = c;
                title_len += 1;
            }
            i += 1;
            continue;
        }
        writer.put(c);
        i += 1;
    }
    writer.flush();
    browser.title[..title_len].copy_from_slice(&title_buf[..title_len]);
    browser.title_len = title_len;
}

fn nav_button(screen: &Screen, x: i64, y: i64, w: i64, h: i64, label: &str, enabled: bool) {
    let (bg, fg) = if enabled { (SURFACE_HI(), TEXT()) } else { (SURFACE(), MUTED()) };
    screen.round_rect(x, y, w, h, h / 2, bg);
    screen.text_centered(x + w / 2, y + h / 2 - 8, label, 2, fg);
}

fn draw_browser(screen: &Screen, browser: &mut Browser, osk: bool) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());

    if browser.web_ok {
        browser.sync_meta();
    }

    screen.round_rect(20, 104, width - 110, 44, 22, SURFACE_HI());
    if browser.editing {
        if browser.url_len == 0 {
            screen.text(36, 118, "enter address", 1, 0x0050_505a);
        } else {
            let shown = browser.url_len.min(40);
            screen.text(36, 118, slice_str(&browser.url[..shown]), 1, TEXT());
        }
    } else if browser.web_ok && browser.page_url_len > 0 {
        let shown = browser.page_url_len.min(44);
        screen.text(36, 118, slice_str(&browser.page_url[..shown]), 1, MUTED());
    } else if browser.url_len > 0 {
        let shown = browser.url_len.min(40);
        screen.text(36, 118, slice_str(&browser.url[..shown]), 1, MUTED());
    } else {
        screen.text(36, 118, "enter address", 1, 0x0050_505a);
    }
    screen.round_rect(width - 84, 104, 64, 44, 22, RED());
    screen.text_centered(width - 52, 116, "GO", 2, TEXT());

    if browser.web_ok {
        nav_button(screen, 20, 152, 56, 36, "<", browser.can_back);
        nav_button(screen, 84, 152, 56, 36, ">", browser.can_fwd);
        nav_button(screen, 148, 152, 56, 36, "R", true);
        let title = slice_str(&browser.page_title[..browser.page_title_len]);
        let shown = title.len().min(28);
        screen.text(216, 162, &title[..shown], 1, TEXT());
    } else if browser.title_len > 0 {
        let shown = browser.title_len.min(44);
        screen.text(20, 162, slice_str(&browser.title[..shown]), 2, TEXT());
    }

    if browser.web_ok {
        screen.fill(19, 195, 442, 1, OUTLINE());
        screen.fill(19, 700, 442, 1, OUTLINE());
        screen.fill(19, 195, 1, 506, OUTLINE());
        screen.fill(460, 195, 1, 506, OUTLINE());
        let seq = k::web_render(false);
        if seq != 0 && seq & 1 == 0 {
            browser.last_seq = seq;
        }
        if browser.last_seq != 0 {
            let bottom = if osk { crate::keyboard::TOP } else { WEB_Y1 };
            browser.blit(bottom);
        } else {
            screen.text_centered(width / 2, 380, "Loading...", 2, MUTED());
        }
    } else {
        let mut y = 196i64;
        let visible_lines = if osk { 15 } else { 34 };
        let end = (browser.scroll + visible_lines).min(browser.line_count);
        for index in browser.scroll..end {
            let length = browser.line_len[index] as usize;
            screen.text(20, y, slice_str(&browser.lines[index][..length]), 1, TEXT());
            y += 14;
        }
        if browser.line_count == 0 {
            screen.text_centered(width / 2, 380, "Type a host and press GO", 2, MUTED());
            if !osk {
                screen.text_centered(width / 2, 416, "Text mode - no host WebView", 1, 0x0050_505a);
            }
        }
    }

    let scroll_y = if osk { 410 } else { 700 };
    if !browser.web_ok {
        screen.round_rect(width - 110, scroll_y, 44, 40, 12, SURFACE_HI());
        screen.text_centered(width - 88, scroll_y + 10, "up", 1, TEXT());
        screen.round_rect(width - 60, scroll_y, 44, 40, 12, SURFACE_HI());
        screen.text_centered(width - 38, scroll_y + 10, "dn", 1, TEXT());
    }
    if !osk {
        screen.text(20, scroll_y + 12, slice_str(&browser.status[..browser.status_len]), 1, MUTED());
    }
}

fn apk_tap(apps: &mut Apps, point: (i64, i64)) {
    if apps.apk.selected.is_some() {
        apps.apk.selected = None;
        return;
    }
    if point.1 >= 120 {
        let index = ((point.1 - 120) / 56) as usize;
        if index < apps.apk.count && point.0 >= 20 && point.0 < 460 {
            let mut path = [0u8; 48];
            let prefix = b"/disk/";
            path[..prefix.len()].copy_from_slice(prefix);
            let len = apps.apk.name_lens[index];
            path[prefix.len()..prefix.len() + len]
                .copy_from_slice(&apps.apk.names[index][..len]);
            let path_str = slice_str(&path[..prefix.len() + len]);
            apps.apk.info = k::apk::ApkInfo::new();
            k::apk::inspect_apk(path_str, &mut apps.apk.info);
            apps.apk.selected = Some(index);
        }
    }
}

fn draw_apk_field(screen: &Screen, x: i64, y: i64, label: &str, value: &[u8], len: usize) {
    screen.text(x, y, label, 1, MUTED());
    screen.text(x + 90, y, slice_str(&value[..len]), 1, TEXT());
}

fn draw_apk(screen: &Screen, apk: &ApkApp) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());

    if let Some(index) = apk.selected {
        let info = &apk.info;
        screen.text(20, 104, slice_str(&apk.names[index][..apk.name_lens[index]]), 2, TEXT());
        if !info.valid {
            screen.text(20, 150, "Could not parse this APK", 1, RED());
            screen.text(20, 700, "tap to go back", 1, MUTED());
            return;
        }
        draw_apk_field(screen, 20, 140, "package", &info.package, info.package_len);
        draw_apk_field(screen, 20, 162, "version", &info.version_name, info.version_name_len);
        draw_apk_field(screen, 20, 184, "label", &info.label, info.label_len);
        screen.text(20, 206, "sdk", 1, MUTED());
        screen.text(110, 206, "min", 1, TEXT());
        screen.text(140, 206, slice_str(&num_buf(info.min_sdk)), 1, TEXT());
        screen.text(180, 206, "target", 1, TEXT());
        screen.text(225, 206, slice_str(&num_buf(info.target_sdk)), 1, TEXT());

        let mut y = 238i64;
        screen.text(20, y, "PERMISSIONS", 1, RED());
        y += 20;
        for i in 0..info.permission_count {
            if y > 560 {
                break;
            }
            let full = &info.permissions[i][..info.permission_lens[i]];
            let short = full.strip_prefix(b"android.permission.").unwrap_or(full);
            screen.text(28, y, slice_str(short), 1, TEXT());
            y += 17;
        }
        y += 8;
        screen.text(20, y, "ACTIVITIES", 1, RED());
        y += 20;
        for i in 0..info.activity_count {
            if y > 640 {
                break;
            }
            screen.text(28, y, slice_str(&info.activities[i][..info.activity_lens[i]]), 1, TEXT());
            y += 17;
        }
        if info.has_dex {
            screen.text(20, 664, "DEX", 1, RED());
            screen.text(60, 664, "classes", 1, MUTED());
            screen.text(115, 664, slice_str(&num_buf(info.dex_classes)), 1, TEXT());
            screen.text(150, 664, "methods", 1, MUTED());
            screen.text(210, 664, slice_str(&num_buf(info.dex_methods)), 1, TEXT());
        }
        screen.text(20, 700, "inspection only — tap to go back", 1, MUTED());
        return;
    }

    screen.text(20, 104, "APK files on /disk", 2, TEXT());
    if apk.count == 0 {
        screen.text_centered(width / 2, 380, "No .apk files found", 2, MUTED());
        screen.text_centered(width / 2, 416, "Insert one via the Android app", 1, 0x0050_505a);
        return;
    }
    for i in 0..apk.count {
        let y = 120 + i as i64 * 56;
        screen.round_rect(20, y, width - 40, 48, 16, SURFACE());
        screen.text(36, y + 16, slice_str(&apk.names[i][..apk.name_lens[i]]), 2, TEXT());
    }
}

fn dvm_tap(apps: &mut Apps, point: (i64, i64)) {
    if apps.dvm.selected.is_some() {
        apps.dvm.selected = None;
        return;
    }
    if point.1 >= 120 {
        let index = ((point.1 - 120) / 56) as usize;
        if index < apps.dvm.count && point.0 >= 20 && point.0 < 460 {
            let mut path = [0u8; 48];
            let prefix = b"/disk/";
            path[..prefix.len()].copy_from_slice(prefix);
            let len = apps.dvm.name_lens[index];
            path[prefix.len()..prefix.len() + len]
                .copy_from_slice(&apps.dvm.names[index][..len]);
            let path_str = slice_str(&path[..prefix.len() + len]);
            apps.dvm.ok = k::dvm_run_file(path_str);
            apps.dvm.out_len = k::dvm::take_output(&mut apps.dvm.output);
            k::print_bytes(&apps.dvm.output[..apps.dvm.out_len]);
            let e = k::dvm::last_error().as_bytes();
            let el = e.len().min(48);
            apps.dvm.err[..el].copy_from_slice(&e[..el]);
            apps.dvm.err_len = el;
            apps.dvm.selected = Some(index);
        }
    }
}

fn draw_dvm(screen: &Screen, dvm: &DvmApp) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());

    if let Some(index) = dvm.selected {
        screen.text(20, 104, slice_str(&dvm.names[index][..dvm.name_lens[index]]), 2, TEXT());
        screen.text(20, 140, "OUTPUT", 1, RED());
        let mut y = 162i64;
        for line in dvm.output[..dvm.out_len].split(|&b| b == b'\n') {
            if y > 660 {
                break;
            }
            if !line.is_empty() {
                screen.text(28, y, slice_str(line), 1, TEXT());
            }
            y += 20;
        }
        if dvm.ok {
            screen.text(20, 684, "executed on mini-Dalvik", 1, MUTED());
        } else {
            screen.text(20, 684, slice_str(&dvm.err[..dvm.err_len]), 1, RED());
        }
        screen.text(20, 704, "tap to go back", 1, MUTED());
        return;
    }

    screen.text(20, 104, "DEX files on /disk", 2, TEXT());
    if dvm.count == 0 {
        screen.text_centered(width / 2, 380, "No .dex files found", 2, MUTED());
        screen.text_centered(width / 2, 416, "Compile Java with javac + d8", 1, 0x0050_505a);
        return;
    }
    for i in 0..dvm.count {
        let y = 120 + i as i64 * 56;
        screen.round_rect(20, y, width - 40, 48, 16, SURFACE());
        screen.text(36, y + 16, slice_str(&dvm.names[i][..dvm.name_lens[i]]), 2, TEXT());
    }
}

fn num_buf(value: u32) -> [u8; 12] {
    let mut buf = [0u8; 12];
    let mut tmp = [0u8; 12];
    let mut n = value;
    let mut len = 0;
    if n == 0 {
        buf[0] = b'0';
        return buf;
    }
    while n > 0 && len < 12 {
        tmp[len] = b'0' + (n % 10) as u8;
        n /= 10;
        len += 1;
    }
    let mut i = 0;
    while len > 0 {
        len -= 1;
        buf[i] = tmp[len];
        i += 1;
    }
    buf
}

// ---- System monitor --------------------------------------------------------

fn monitor_tap(apps: &mut Apps, point: (i64, i64)) {
    let m = &mut apps.monitor;
    if m.selected.is_some() {
        if point.1 >= 560 && point.1 <= 612 && point.0 >= 120 && point.0 <= 360 {
            m.kill_selected();
        } else {
            m.selected = None;
            m.kill_len = 0;
        }
        return;
    }
    if point.1 >= 440 && point.1 <= 472 {
        if point.0 >= 360 && point.0 <= 404 {
            m.scroll = m.scroll.saturating_sub(5);
            return;
        }
        if point.0 >= 412 && point.0 <= 456 {
            if m.scroll + 10 < m.proc_count {
                m.scroll += 5;
            }
            return;
        }
    }
    if point.1 >= 476 && point.1 < 756 && point.0 >= 20 && point.0 < 460 {
        let row = ((point.1 - 476) / 28) as usize;
        if row < 10 {
            let index = m.scroll + row;
            if index < m.proc_count {
                m.selected = Some(m.procs[index].pid);
                m.kill_len = 0;
            }
        }
    }
}

fn monitor_bar(screen: &Screen, x: i64, y: i64, w: i64, h: i64, permille: i64, color: u32) {
    screen.round_rect(x, y, w, h, h / 2, SURFACE_MAX());
    let fill = (w * permille.clamp(0, 1000) / 1000).max(if permille > 0 { h } else { 0 });
    if fill > 0 {
        screen.round_rect(x, y, fill, h, h / 2, color);
    }
}

fn kb_text(kb: u64, buf: &mut [u8; 24]) -> &str {
    let mut used = 0;
    let mut num = [0u8; 20];
    if kb >= 1024 * 1024 {
        push_str(buf, &mut used, format_tenths((kb / 1024) as i64 * 10 / 1024, &mut num).as_bytes());
        push_str(buf, &mut used, b" GB");
    } else if kb >= 1024 {
        push_str(buf, &mut used, unsigned(kb / 1024, &mut num).as_bytes());
        push_str(buf, &mut used, b" MB");
    } else {
        push_str(buf, &mut used, unsigned(kb, &mut num).as_bytes());
        push_str(buf, &mut used, b" KB");
    }
    slice_str(&buf[..used])
}

fn draw_monitor(screen: &Screen, m: &mut Monitor) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    let now = k::uptime_ms();
    if now.saturating_sub(m.last_refresh) >= 1000 {
        m.refresh();
    }

    if let Some(pid) = m.selected {
        let Some(p) = m.find(pid).copied() else {
            m.selected = None;
            return;
        };
        screen.round_rect(20, 104, width - 40, 440, 26, SURFACE());
        let name = slice_str(&p.name[..p.name_len]);
        screen.text(40, 124, name, 3, TEXT());
        let mut buf = [0u8; 20];
        screen.text(40, 180, "PID", 1, MUTED());
        screen.text(150, 180, unsigned(p.pid, &mut buf), 2, TEXT());
        screen.text(40, 216, "Parent", 1, MUTED());
        screen.text(150, 216, unsigned(p.ppid, &mut buf), 2, TEXT());
        screen.text(40, 252, "Memory", 1, MUTED());
        let mut mem = [0u8; 24];
        screen.text(150, 252, kb_text(p.ram_kb, &mut mem), 2, TEXT());
        monitor_bar(screen, 40, 288, width - 80, 12, (p.ram_kb * 1000 / m.info[1].max(1)) as i64, RED());
        screen.text(40, 316, "CPU", 1, MUTED());
        let mut cpu = [0u8; 24];
        let mut used = 0;
        push_str(&mut cpu, &mut used, format_tenths(p.cpu_x10 as i64, &mut buf).as_bytes());
        push_str(&mut cpu, &mut used, b" %");
        screen.text(150, 316, slice_str(&cpu[..used]), 2, TEXT());
        screen.text(40, 352, "State", 1, MUTED());
        let state = match p.state { 0 => "Running", 1 => "Sleeping", 2 => "Zombie", _ => "Unknown" };
        screen.text(150, 352, state, 2, TEXT());
        screen.text(40, 396, "Cmdline", 1, MUTED());
        screen.text(150, 396, "not exposed by the kernel", 1, MUTED());
        screen.text(40, 430, "Memory breakdown: resident set only", 1, MUTED());
        screen.text(40, 452, "(no per-region map available)", 1, MUTED());
        button(screen, 120, 560, 240, 52, "TERMINATE", RED(), TEXT());
        if m.kill_len > 0 {
            screen.text_centered(width / 2, 636, slice_str(&m.kill_msg[..m.kill_len]), 1, GREEN());
        }
        screen.text(40, 700, "Tap elsewhere to go back", 1, MUTED());
        return;
    }

    if !m.has_info {
        screen.text_centered(width / 2, 300, "System stats unavailable", 2, MUTED());
        screen.text_centered(width / 2, 340, "Needs the libOS host (SYS_SYSINFO)", 1, MUTED());
    }

    let cpu_permille = (m.info[0] as i64 * 100 / 10).min(1000);
    screen.round_rect(20, 104, width - 40, 84, 22, SURFACE());
    screen.text(36, 116, "CPU", 1, MUTED());
    let mut buf = [0u8; 20];
    let mut line = [0u8; 32];
    let mut used = 0;
    push_str(&mut line, &mut used, format_tenths(m.info[0] as i64, &mut buf).as_bytes());
    push_str(&mut line, &mut used, b"%");
    screen.text(width - 36 - Screen::text_width(slice_str(&line[..used]), 2), 112, slice_str(&line[..used]), 2, TEXT());
    monitor_bar(screen, 36, 142, 240, 12, cpu_permille, RED());
    let hist_x = 300i64;
    let hist_w = 144i64;
    for i in 0..m.hist_len {
        let x = hist_x + (i as i64 * hist_w) / 48;
        let h = (m.cpu_hist[i] as i64 * 30 / 100).max(1);
        screen.fill(x, 172 - h, 2, h, RED_DIM());
    }
    screen.text(36, 164, "load history", 1, MUTED());

    screen.round_rect(20, 196, width - 40, 64, 20, SURFACE());
    screen.text(36, 206, "RAM", 1, MUTED());
    let mut ram = [0u8; 48];
    let mut rn = 0;
    let mut t1 = [0u8; 24];
    let mut t2 = [0u8; 24];
    push_str(&mut ram, &mut rn, kb_text(m.info[2], &mut t1).as_bytes());
    push_str(&mut ram, &mut rn, b" / ");
    push_str(&mut ram, &mut rn, kb_text(m.info[1], &mut t2).as_bytes());
    screen.text(width - 36 - Screen::text_width(slice_str(&ram[..rn]), 1), 206, slice_str(&ram[..rn]), 1, TEXT());
    let ram_permille = if m.info[1] > 0 { (m.info[2] * 1000 / m.info[1]) as i64 } else { 0 };
    monitor_bar(screen, 36, 234, width - 72, 12, ram_permille, 0x00b8_c4ff);

    screen.round_rect(20, 268, width - 40, 64, 20, SURFACE());
    screen.text(36, 278, "STORAGE", 1, MUTED());
    let mut sto = [0u8; 48];
    let mut sn = 0;
    push_str(&mut sto, &mut sn, kb_text(m.info[4], &mut t1).as_bytes());
    push_str(&mut sto, &mut sn, b" / ");
    push_str(&mut sto, &mut sn, kb_text(m.info[3], &mut t2).as_bytes());
    screen.text(width - 36 - Screen::text_width(slice_str(&sto[..sn]), 1), 278, slice_str(&sto[..sn]), 1, TEXT());
    let sto_permille = if m.info[3] > 0 { (m.info[4] * 1000 / m.info[3]) as i64 } else { 0 };
    monitor_bar(screen, 36, 306, width - 72, 12, sto_permille, GREEN());

    let mut row1 = [0u8; 96];
    let mut u1 = 0;
    push_str(&mut row1, &mut u1, b"Battery ");
    push_str(&mut row1, &mut u1, unsigned(m.info[7], &mut buf).as_bytes());
    push_str(&mut row1, &mut u1, if m.info[8] != 0 { b"% charging" } else { b"%" });
    push_str(&mut row1, &mut u1, b"   Uptime ");
    let up = m.info[5];
    push_str(&mut row1, &mut u1, unsigned(up / 3600, &mut buf).as_bytes());
    push_str(&mut row1, &mut u1, b"h ");
    push_str(&mut row1, &mut u1, unsigned((up / 60) % 60, &mut buf).as_bytes());
    push_str(&mut row1, &mut u1, b"m");
    screen.text(24, 348, slice_str(&row1[..u1]), 1, TEXT());
    let mut row2 = [0u8; 96];
    let mut u2 = 0;
    push_str(&mut row2, &mut u2, b"Processes ");
    push_str(&mut row2, &mut u2, unsigned(m.info[6], &mut buf).as_bytes());
    push_str(&mut row2, &mut u2, b"   FPS ");
    push_str(&mut row2, &mut u2, format_tenths(m.fps_x10 as i64, &mut buf).as_bytes());
    screen.text(24, 372, slice_str(&row2[..u2]), 1, TEXT());

    screen.text(24, 400, "ACTIVE APPS", 1, MUTED());
    let mut apps_line = [0u8; 128];
    let mut an = 0;
    for i in 0..m.active_count {
        if i > 0 {
            push_str(&mut apps_line, &mut an, b"  ");
        }
        push_str(&mut apps_line, &mut an, &m.active[i][..m.active_len[i] as usize]);
    }
    if an == 0 {
        push_str(&mut apps_line, &mut an, b"(none yet)");
    }
    let shown = an.min(56);
    screen.text(24, 420, slice_str(&apps_line[..shown]), 1, TEXT());

    screen.text(24, 452, "PROCESSES", 1, MUTED());
    button(screen, 360, 440, 44, 30, "up", SURFACE_HI(), MUTED());
    button(screen, 412, 440, 44, 30, "dn", SURFACE_HI(), MUTED());
    if m.proc_count == 0 {
        screen.text_centered(width / 2, 560, "No process data (host only)", 1, MUTED());
        return;
    }
    for row in 0..10usize {
        let index = m.scroll + row;
        if index >= m.proc_count {
            break;
        }
        let p = &m.procs[index];
        let y = 476 + row as i64 * 28;
        screen.round_rect(20, y, width - 40, 26, 10, if row % 2 == 0 { SURFACE() } else { BLACK() });
        let mut pid = [0u8; 20];
        screen.text(28, y + 8, unsigned(p.pid, &mut pid), 1, MUTED());
        let name = slice_str(&p.name[..p.name_len]);
        let nlen = name.len().min(18);
        screen.text(76, y + 8, &name[..nlen], 1, TEXT());
        let mut mem = [0u8; 24];
        let mem_text = kb_text(p.ram_kb, &mut mem);
        screen.text(250, y + 8, mem_text, 1, MUTED());
        let mut cpu = [0u8; 20];
        let mut cn = 0;
        push_str(&mut cpu, &mut cn, format_tenths(p.cpu_x10 as i64, &mut pid).as_bytes());
        push_str(&mut cpu, &mut cn, b"%");
        screen.text(340, y + 8, slice_str(&cpu[..cn]), 1, MUTED());
        let state = match p.state { 0 => "R", 1 => "S", 2 => "Z", _ => "?" };
        screen.text(430, y + 8, state, 1, if p.state == 0 { GREEN() } else { MUTED() });
    }
}

// ---- Plugin manager -------------------------------------------------------

const PLG_LIST_Y: i64 = 180;
const PLG_ROW_H: i64 = 60;
const PLG_ACTION_Y: i64 = 672;

fn plugin_type_badge(ptype: u8) -> &'static str {
    match ptype {
        TYPE_SERVICE => "SVC",
        TYPE_UI => "UI",
        _ => "APP",
    }
}

fn draw_plugins(screen: &Screen, plugins: &Plugins) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());

    if plugins.view == 1 {
        button(screen, 20, 104, 100, 40, "Back", SURFACE_HI(), TEXT());
        button(screen, 128, 104, 120, 40, "Picker", SURFACE_HI(), MUTED());
        button(screen, 356, 104, 100, 40, "Rescan", SURFACE_HI(), MUTED());
        screen.text(24, 156, "Install from /disk/import", 1, MUTED());
        if plugins.status_len > 0 {
            let shown = plugins.status_len.min(40);
            screen.text(24, 712, slice_str(&plugins.status[..shown]), 1, GREEN());
        }
        if plugins.import_count == 0 {
            screen.text_centered(width / 2, 380, "No .kplugin packages found", 2, MUTED());
            screen.text_centered(width / 2, 416, "Drop a plugin dir into /disk/import", 1, MUTED());
            return;
        }
        for row in 0..PLUGIN_ROWS {
            let i = row;
            if i >= plugins.import_count {
                break;
            }
            let y = PLG_LIST_Y + row as i64 * PLG_ROW_H;
            screen.round_rect(20, y, width - 40, 54, 18, SURFACE());
            let path = slice_str(&plugins.import_paths[i][..plugins.import_lens[i]]);
            let base = match path.rfind('/') {
                Some(p) => &path[p + 1..],
                None => path,
            };
            let shown = base.len().min(26);
            screen.text(40, y + 12, &base[..shown], 2, TEXT());
            screen.text(40, y + 38, "plugin package", 1, MUTED());
            button(screen, 330, y + 7, 110, 40, "Install", RED(), TEXT());
        }
        return;
    }

    button(screen, 20, 104, 100, 40, "Import", RED_DIM(), TEXT());
    button(screen, 128, 104, 120, 40, "Services", SURFACE_HI(), MUTED());
    button(screen, 356, 104, 100, 40, "Refresh", SURFACE_HI(), MUTED());
    let port = kplugin::api_port();
    let mut port_buf = [0u8; 20];
    let port_text = unsigned(port as u64, &mut port_buf);
    let mut line = [0u8; 32];
    let mut used = 0;
    push_str(&mut line, &mut used, b"API :");
    push_str(&mut line, &mut used, port_text.as_bytes());
    screen.text(256, 116, slice_str(&line[..used]), 1, if port > 0 { GREEN() } else { MUTED() });

    if plugins.status_len > 0 {
        let shown = plugins.status_len.min(52);
        screen.text(24, 156, slice_str(&plugins.status[..shown]), 1, GREEN());
    } else {
        screen.text(24, 156, "Tap a plugin to select it", 1, MUTED());
    }

    if plugins.count == 0 {
        screen.text_centered(width / 2, 380, "No plugins installed", 2, MUTED());
        screen.text_centered(width / 2, 416, "Import a .kplugin to get started", 1, MUTED());
    }
    for row in 0..PLUGIN_ROWS {
        let i = plugins.scroll + row;
        if i >= plugins.count {
            break;
        }
        let info = &plugins.items[i];
        let y = PLG_LIST_Y + row as i64 * PLG_ROW_H;
        let selected = plugins.selected == Some(i);
        screen.round_rect(20, y, width - 40, 54, 18, if selected { SURFACE_HI() } else { SURFACE() });
        let name = info.name_str();
        let shown = name.len().min(22);
        screen.text(40, y + 12, &name[..shown], 2, TEXT());
        let mut sub = [0u8; 48];
        let mut sn = 0;
        push_str(&mut sub, &mut sn, b"v");
        push_str(&mut sub, &mut sn, info.version_str().as_bytes());
        push_str(&mut sub, &mut sn, b"  ");
        push_str(&mut sub, &mut sn, plugin_type_badge(info.ptype).as_bytes());
        screen.text(40, y + 38, slice_str(&sub[..sn]), 1, MUTED());
        let on = info.enabled;
        screen.round_rect(width - 104, y + 15, 64, 24, 12, if on { RED_DIM() } else { SURFACE_MAX() });
        screen.text_centered(width - 72, y + 19, if on { "ON" } else { "OFF" }, 1, if on { TEXT() } else { MUTED() });
    }

    if let Some(i) = plugins.selected {
        let info = &plugins.items[i];
        let toggle_label = if info.enabled { "Disable" } else { "Enable" };
        button(screen, 20, PLG_ACTION_Y, 130, 44, toggle_label, SURFACE_HI(), TEXT());
        let (mid_label, mid_bg) = match info.ptype {
            TYPE_APP => ("Run", RED()),
            TYPE_UI => ("Apply", RED()),
            _ => ("Start", SURFACE_HI()),
        };
        button(screen, 160, PLG_ACTION_Y, 130, 44, mid_label, mid_bg, TEXT());
        let del_label = if plugins.confirm_uninstall { "SURE?" } else { "Uninstall" };
        let del_bg = if plugins.confirm_uninstall { RED() } else { RED_DIM() };
        button(screen, 300, PLG_ACTION_Y, 140, 44, del_label, del_bg, TEXT());
    }

    button(screen, 20, 728, 120, 36, "UP", SURFACE_HI(), MUTED());
    button(screen, 340, 728, 120, 36, "DOWN", SURFACE_HI(), MUTED());
    let mut buf = [0u8; 20];
    let count = unsigned(plugins.count as u64, &mut buf);
    screen.text_centered(width / 2, 738, count, 1, MUTED());
}

fn plugins_tap(screen: &Screen, apps: &mut Apps, point: (i64, i64)) {
    let plugins = &mut apps.plugins;
    if plugins.view == 1 {
        if point.1 >= 104 && point.1 <= 144 {
            if point.0 >= 20 && point.0 <= 120 {
                plugins.view = 0;
                return;
            }
            if point.0 >= 128 && point.0 <= 248 {
                if k::import_file("/disk/import") {
                    plugins.set_status("Picker opened - Rescan after");
                } else {
                    plugins.set_status("Import unavailable on this host");
                }
                plugins.scan_imports();
                return;
            }
            if point.0 >= 356 && point.0 <= 456 {
                plugins.scan_imports();
                return;
            }
        }
        if point.1 >= PLG_LIST_Y && point.0 >= 20 && point.0 < 460 {
            let row = ((point.1 - PLG_LIST_Y) / PLG_ROW_H) as usize;
            if row < PLUGIN_ROWS && row < plugins.import_count {
                plugins.install_import(row);
            }
        }
        return;
    }

    if point.1 >= 104 && point.1 <= 144 {
        if point.0 >= 20 && point.0 <= 120 {
            plugins.view = 1;
            plugins.scan_imports();
            return;
        }
        if point.0 >= 128 && point.0 <= 248 {
            kplugin::start_services();
            plugins.set_status("Services started");
            return;
        }
        if point.0 >= 356 && point.0 <= 456 {
            plugins.refresh();
            return;
        }
    }

    if plugins.selected.is_some() && point.1 >= PLG_ACTION_Y && point.1 <= PLG_ACTION_Y + 44 {
        if point.0 >= 20 && point.0 <= 150 {
            plugins.toggle_selected();
            return;
        }
        if point.0 >= 160 && point.0 <= 290 {
            plugins.primary_action(screen);
            return;
        }
        if point.0 >= 300 && point.0 <= 440 {
            plugins.uninstall_selected();
            return;
        }
    }

    if point.1 >= 728 && point.1 <= 764 {
        if point.0 >= 20 && point.0 <= 140 {
            plugins.scroll = plugins.scroll.saturating_sub(1);
            return;
        }
        if point.0 >= 340 && point.0 <= 460 {
            if plugins.scroll + PLUGIN_ROWS < plugins.count {
                plugins.scroll += 1;
            }
            return;
        }
    }

    if point.1 >= PLG_LIST_Y && point.1 < PLG_LIST_Y + PLUGIN_ROWS as i64 * PLG_ROW_H
        && point.0 >= 20 && point.0 < 460
    {
        let row = ((point.1 - PLG_LIST_Y) / PLG_ROW_H) as usize;
        let index = plugins.scroll + row;
        if row < PLUGIN_ROWS && index < plugins.count {
            plugins.selected = Some(index);
            plugins.confirm_uninstall = false;
        }
    }
}

fn plugins_key(apps: &mut Apps, code: u16) -> bool {
    let plugins = &mut apps.plugins;
    if plugins.view != 0 || plugins.count == 0 {
        return false;
    }
    match code {
        103 => {
            let cur = plugins.selected.unwrap_or(0);
            let next = cur.saturating_sub(1);
            plugins.selected = Some(next);
            if next < plugins.scroll {
                plugins.scroll = next;
            }
            true
        }
        108 => {
            let cur = plugins.selected.unwrap_or(0);
            let next = (cur + 1).min(plugins.count - 1);
            plugins.selected = Some(next);
            if next >= plugins.scroll + PLUGIN_ROWS {
                plugins.scroll = next + 1 - PLUGIN_ROWS;
            }
            true
        }
        28 => {
            if plugins.selected.is_none() {
                plugins.selected = Some(0);
            } else {
                plugins.toggle_selected();
            }
            true
        }
        _ => false,
    }
}

// ---- Marketplace (powered by kpkg) ----------------------------------------

#[derive(Clone, Copy)]
struct MarketItem {
    name: [u8; 32],
    name_len: usize,
    desc: [u8; 64],
    desc_len: usize,
    kind: u8,    // 0 theme, 1 font, 2 app, 3 plugin
    source: u8,  // 0 built-in, 1 repo, 2 local import, 3 installed plugin
    index: usize,
    installed: bool,
}

impl MarketItem {
    fn name_str(&self) -> &str { slice_str(&self.name[..self.name_len]) }
    fn desc_str(&self) -> &str { slice_str(&self.desc[..self.desc_len]) }
}

pub struct Market {
    tab: usize,
    scroll: [usize; 4],
    items: [MarketItem; 40],
    count: usize,
    status: [u8; 64],
    status_len: usize,
    repo_ok: bool,
    import_paths: [[u8; 96]; 8],
    import_lens: [usize; 8],
    import_count: usize,
    plugins: [PluginInfo; 16],
    plugin_count: usize,
}

const EMPTY_ITEM: MarketItem = MarketItem {
    name: [0; 32], name_len: 0, desc: [0; 64], desc_len: 0,
    kind: 0, source: 0, index: 0, installed: false,
};

impl Market {
    pub fn new() -> Self {
        let mut m = Self {
            tab: 0,
            scroll: [0; 4],
            items: [EMPTY_ITEM; 40],
            count: 0,
            status: [0; 64],
            status_len: 0,
            repo_ok: false,
            import_paths: [[0; 96]; 8],
            import_lens: [0; 8],
            import_count: 0,
            plugins: [EMPTY_PLUGIN; 16],
            plugin_count: 0,
        };
        m.set_status("Themes, fonts, apps & plugins — kpkg");
        m
    }

    fn set_status(&mut self, s: &str) {
        let b = s.as_bytes();
        self.status_len = b.len().min(64);
        self.status[..self.status_len].copy_from_slice(&b[..self.status_len]);
    }

    fn push(&mut self, name: &str, desc: &str, kind: u8, source: u8, index: usize, installed: bool) {
        if self.count >= self.items.len() { return; }
        let it = &mut self.items[self.count];
        it.name_len = name.len().min(32);
        it.name[..it.name_len].copy_from_slice(&name.as_bytes()[..it.name_len]);
        it.desc_len = desc.len().min(64);
        it.desc[..it.desc_len].copy_from_slice(&desc.as_bytes()[..it.desc_len]);
        it.kind = kind; it.source = source; it.index = index; it.installed = installed;
        self.count += 1;
    }

    fn refresh(&mut self) {
        self.count = 0;
        let _ = k::mkdir("/disk/themes");
        let _ = k::mkdir("/disk/fonts");
        for i in 0..crate::theme::theme_count() {
            self.push(crate::theme::theme_name(i), "Built-in theme", 0, 0, i, true);
        }
        for i in 0..crate::theme::font_count() {
            if i == 4 && !crate::font_store::valid() { continue; }
            self.push(crate::theme::font_name(i), "Built-in font", 1, 0, i, true);
        }
        let mut ord = 0usize;
        k::kpkg::each_package(|p| {
            let kind = match p.kind_str() { "theme" => 0, "font" => 1, _ => 2 };
            let mut buf = [0u8; 96];
            let path = asset_path(kind, p.name_str(), &mut buf);
            let installed = k::stat(path).is_some();
            self.push(p.name_str(), p.desc_str(), kind, 1, ord, installed);
            ord += 1;
        });
        self.plugin_count = kplugin::list(&mut self.plugins);
        for i in 0..self.plugin_count {
            let p = self.plugins[i];
            let mut desc = [0u8; 64];
            let mut dn = 0;
            push_str(&mut desc, &mut dn, b"v");
            push_str(&mut desc, &mut dn, p.version_str().as_bytes());
            push_str(&mut desc, &mut dn, b" ");
            push_str(&mut desc, &mut dn, p.type_str().as_bytes());
            push_str(&mut desc, &mut dn, if p.enabled { b" - enabled" } else { b" - disabled" });
            self.push(p.name_str(), slice_str(&desc[..dn]), 3, 3, i, true);
        }
        self.scan_imports();
        self.repo_ok = k::kpkg::repo_cached();
        for t in 0..4 {
            let n = self.kind_count(t as u8);
            if self.scroll[t] + 6 > n { self.scroll[t] = n.saturating_sub(6); }
        }
    }

    fn scan_imports(&mut self) {
        let _ = k::mkdir("/disk/import");
        let mut kept = 0usize;
        for i in 0..self.count {
            if self.items[i].source != 2 {
                self.items[kept] = self.items[i];
                kept += 1;
            }
        }
        self.count = kept;
        self.import_count = 0;
        let mut buffer = [0u8; 1024];
        let Some(n) = k::listdir("/disk/import", &mut buffer) else { return };
        for name in buffer[..n].split(|&b| b == 0) {
            if name.is_empty() || name.first() == Some(&b'.') || self.import_count >= 8 {
                continue;
            }
            let is_dir = name.last() == Some(&b'/');
            let base = if is_dir { &name[..name.len() - 1] } else { name };
            if base.is_empty() || base.len() > 40 || self.count >= self.items.len() {
                continue;
            }
            let kind = if is_dir {
                let mut probe = [0u8; 96];
                let mut pn = 0;
                push_str(&mut probe, &mut pn, b"/disk/import/");
                push_str(&mut probe, &mut pn, base);
                push_str(&mut probe, &mut pn, b"/plugin.json");
                if k::stat(slice_str(&probe[..pn])).is_none() {
                    continue;
                }
                3u8
            } else if name.ends_with(b".ktheme") {
                0u8
            } else if name.ends_with(b".kfont") {
                1u8
            } else if name.ends_with(b".kpkg") {
                2u8
            } else if name.ends_with(b".kplugin") {
                3u8
            } else {
                continue;
            };
            let slot = self.import_count;
            let mut path = [0u8; 96];
            let mut used = 0;
            push_str(&mut path, &mut used, b"/disk/import/");
            push_str(&mut path, &mut used, base);
            self.import_paths[slot][..used].copy_from_slice(&path[..used]);
            self.import_lens[slot] = used;
            self.import_count += 1;
            let stem = if kind == 3 {
                base
            } else {
                match base.iter().rposition(|&b| b == b'.') {
                    Some(i) if i > 0 => &base[..i],
                    _ => base,
                }
            };
            let stem_str = slice_str(stem);
            let installed = if kind == 3 {
                false
            } else {
                let mut dest = [0u8; 96];
                let dest_path = asset_path(kind, stem_str, &mut dest);
                k::stat(dest_path).is_some()
            };
            let desc = if kind == 3 { "Plugin package - install" } else { "Local file - validate & install" };
            self.push(stem_str, desc, kind, 2, slot, installed);
        }
    }

    fn import_picker(&mut self) {
        if k::import_file("/disk/import") {
            self.set_status("Picker opened - Refresh after importing");
        } else {
            self.set_status("Import unavailable on this host");
        }
        self.scan_imports();
    }

    fn kind_count(&self, kind: u8) -> usize {
        self.items[..self.count].iter().filter(|i| i.kind == kind).count()
    }

    fn filtered(&self, kind: u8, pos: usize) -> Option<MarketItem> {
        let mut j = 0;
        for i in 0..self.count {
            let it = self.items[i];
            if it.kind == kind {
                if j == pos { return Some(it); }
                j += 1;
            }
        }
        None
    }

    fn action(&mut self, it: MarketItem) {
        if it.source == 2 {
            self.action_import(it);
            return;
        }
        if it.source == 3 {
            if it.index < self.plugin_count {
                let p = self.plugins[it.index];
                let on = !p.enabled;
                kplugin::set_enabled(p.id_str(), on);
                self.plugins[it.index].enabled = on;
                self.set_status(if on { "Plugin enabled" } else { "Plugin disabled" });
            }
            self.refresh();
            return;
        }
        let name = it.name_str();
        match it.kind {
            0 => {
                if it.source == 0 {
                    crate::theme::apply_theme(it.index);
                } else {
                    let mut buf = [0u8; 96];
                    let path = asset_path(0, name, &mut buf);
                    if !it.installed { k::kpkg::install_to(name, path); }
                    crate::theme::load_theme_file(path, name);
                }
                save_theme_cfg();
                self.set_status("Theme applied");
            }
            1 => {
                if it.source == 0 {
                    crate::theme::apply_font(it.index);
                } else {
                    let mut buf = [0u8; 96];
                    let path = asset_path(1, name, &mut buf);
                    if !it.installed { k::kpkg::install_to(name, path); }
                    crate::font_store::load(path, name);
                    crate::theme::apply_font(4);
                }
                save_theme_cfg();
                self.set_status("Font applied");
            }
            _ => {
                let mut buf = [0u8; 96];
                let path = asset_path(2, name, &mut buf);
                if k::kpkg::install_to(name, path) {
                    self.set_status("Installed — run it in the Terminal");
                } else {
                    self.set_status("Install failed (repo reachable?)");
                }
            }
        }
        self.refresh();
    }
}

impl Market {
    fn action_import(&mut self, it: MarketItem) {
        if it.index >= self.import_count {
            self.set_status("Import slot missing - Refresh");
            return;
        }
        let path = slice_str(&self.import_paths[it.index][..self.import_lens[it.index]]);
        let result = k::kpkg::import_local(path);
        if !result.ok {
            self.set_status(result.msg_str());
            self.refresh();
            return;
        }
        match result.kind {
            0 => {
                if crate::theme::load_theme_file(result.dest_str(), result.name_str()) {
                    save_theme_cfg();
                    self.set_status("Theme imported & applied");
                } else {
                    self.set_status("Installed, but palette parse failed");
                }
            }
            1 => {
                if crate::font_store::load(result.dest_str(), result.name_str()) {
                    crate::theme::apply_font(4);
                    save_theme_cfg();
                    self.set_status("Font imported & applied");
                } else {
                    self.set_status("Installed, but font parse failed");
                }
            }
            3 => {
                self.set_status("Plugin installed - manage it in Plugins");
            }
            _ => {
                self.set_status("App installed - run it in Files/Terminal");
            }
        }
        self.refresh();
    }
}

fn asset_path<'a>(kind: u8, name: &str, buf: &'a mut [u8; 96]) -> &'a str {
    let (dir, ext) = match kind {
        0 => ("/disk/themes/", ".theme"),
        1 => ("/disk/fonts/", ".kfont"),
        _ => ("/disk/bin/", ""),
    };
    let mut n = 0usize;
    for piece in [dir.as_bytes(), name.as_bytes(), ext.as_bytes()] {
        let take = piece.len().min(buf.len() - n);
        buf[n..n + take].copy_from_slice(&piece[..take]);
        n += take;
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

fn item_active(it: &MarketItem) -> bool {
    match (it.kind, it.source) {
        (0, 0) => !crate::theme::is_custom() && crate::theme::active_theme() == it.index,
        (0, _) => crate::theme::is_custom() && crate::theme::custom_name() == it.name_str(),
        (1, 0) => crate::theme::active_font() == it.index,
        (1, _) => crate::theme::active_font() == 4 && crate::font_store::name() == it.name_str(),
        _ => false,
    }
}

fn market_tap(apps: &mut Apps, point: (i64, i64)) {
    let m = &mut apps.market;
    if point.1 >= 100 && point.1 <= 142 {
        m.tab = ((point.0 - 20) / 112).clamp(0, 3) as usize;
        return;
    }
    if point.1 >= 150 && point.1 <= 186 {
        if point.0 >= 200 && point.0 < 326 {
            m.import_picker();
            return;
        }
        if point.0 >= 330 {
            if k::kpkg::update_quiet("") {
                m.set_status("Repo updated");
            } else {
                m.set_status("Fetch failed — built-ins still available");
            }
            m.refresh();
            return;
        }
    }
    if point.1 >= 760 {
        if point.0 < 160 {
            m.scroll[m.tab] = m.scroll[m.tab].saturating_sub(1);
        } else if point.0 > 320 {
            let n = m.kind_count(m.tab as u8);
            if m.scroll[m.tab] + 6 < n { m.scroll[m.tab] += 1; }
        }
        return;
    }
    if point.1 >= 196 && point.0 >= 20 && point.0 < 460 {
        let row = ((point.1 - 196) / 96) as usize;
        if row < 6 {
            if let Some(it) = m.filtered(m.tab as u8, m.scroll[m.tab] + row) {
                m.action(it);
            }
        }
    }
}

fn draw_market(screen: &Screen, m: &Market) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());

    let tabs = ["Themes", "Fonts", "Apps", "Plugins"];
    for t in 0..4 {
        let x = 20 + t as i64 * 112;
        let w = if t == 3 { 116 } else { 108 };
        let active = m.tab == t;
        screen.round_rect(x, 100, w, 42, 21, if active { RED_DIM() } else { SURFACE() });
        screen.text_centered(x + w / 2, 110, tabs[t], 2, if active { TEXT() } else { MUTED() });
    }

    let status_shown = m.status_len.min(32);
    screen.text(24, 162, slice_str(&m.status[..status_shown]), 1, MUTED());
    button(screen, 200, 150, 120, 36, "Import", RED_DIM(), TEXT());
    button(screen, 330, 150, 126, 36, "Refresh", SURFACE_HI(), TEXT());

    let kind = m.tab as u8;
    let mut row = 0i64;
    let mut skipped = 0usize;
    for i in 0..m.count {
        let it = m.items[i];
        if it.kind != kind { continue; }
        if skipped < m.scroll[m.tab] { skipped += 1; continue; }
        if row >= 6 { break; }
        let y = 196 + row * 96;
        screen.round_rect(20, y, width - 40, 88, 20, SURFACE());
        let name = it.name_str();
        let nlen = name.len().min(22);
        screen.text(40, y + 14, &name[..nlen], 2, TEXT());
        let desc = it.desc_str();
        let dlen = desc.len().min(44);
        screen.text(40, y + 46, &desc[..dlen], 1, MUTED());
        let tag = match (it.kind, it.source) {
            (_, 2) => "LOCAL",
            (0, _) => "THEME",
            (1, _) => "FONT",
            (3, _) => "PLUGIN",
            _ => "APP",
        };
        screen.text(40, y + 66, tag, 1, OUTLINE());

        let active = item_active(&it);
        let (label, bg, fg) = if it.source == 2 {
            if it.installed { ("Installed", SURFACE_HI(), MUTED()) } else { ("Import", RED(), TEXT()) }
        } else if it.source == 3 {
            let on = it.index < m.plugin_count && m.plugins[it.index].enabled;
            if on { ("On", RED_DIM(), GREEN()) } else { ("Off", SURFACE_HI(), MUTED()) }
        } else if it.kind == 2 {
            if it.installed { ("Installed", SURFACE_HI(), MUTED()) } else { ("Install", SURFACE_HI(), TEXT()) }
        } else if active {
            ("Active", SURFACE_MAX(), GREEN())
        } else if it.installed || it.source == 0 {
            ("Apply", RED(), TEXT())
        } else {
            ("Install", SURFACE_HI(), TEXT())
        };
        button(screen, 330, y + 24, 110, 40, label, bg, fg);
        row += 1;
    }

    if row == 0 {
        screen.text_centered(width / 2, 300, "Nothing here yet", 2, MUTED());
        screen.text_centered(width / 2, 336, "Tap Refresh to fetch the kpkg repo", 1, MUTED());
    }

    button(screen, 20, 760, 120, 32, "UP", SURFACE_HI(), MUTED());
    button(screen, 340, 760, 120, 32, "DOWN", SURFACE_HI(), MUTED());
    let total = m.kind_count(kind);
    let mut buf = [0u8; 20];
    let pos = unsigned((m.scroll[m.tab] + 1) as u64, &mut buf);
    let mut buf2 = [0u8; 20];
    let tot = unsigned(total as u64, &mut buf2);
    let mut line = [0u8; 48];
    let mut ln = 0;
    for piece in [pos.as_bytes(), b" / ", tot.as_bytes()] {
        let take = piece.len().min(line.len() - ln);
        line[ln..ln + take].copy_from_slice(&piece[..take]);
        ln += take;
    }
    screen.text_centered(width / 2, 768, slice_str(&line[..ln]), 1, MUTED());
}

/// Host-only test hook: parse a canned weather JSON body (mode 1 = HKO
/// rhrread, 2 = Open-Meteo current, 3 = HKO fnd) into `out`.
#[cfg(not(target_os = "none"))]
pub fn weather_parse_test(data: &[u8], mode: u64, out: &mut [i64]) -> bool {
    let mut weather = Weather::new();
    match mode {
        1 => {
            if !parse_hko(&mut weather, data) {
                return false;
            }
            out[0] = weather.temp;
            out[1] = weather.humidity;
            out[2] = weather.wind;
            out[3] = weather.feels;
            out[4] = weather.cond_len as i64;
            out[5] = weather.provider as i64;
            true
        }
        2 => {
            if !parse_open_meteo(&mut weather, data) {
                return false;
            }
            out[0] = weather.temp;
            out[1] = weather.humidity;
            out[2] = weather.wind;
            out[3] = weather.feels;
            out[4] = weather.code as i64;
            out[5] = weather.provider as i64;
            true
        }
        3 => {
            parse_fnd(&mut weather, data);
            out[0] = weather.forecast_count as i64;
            if weather.forecast_count > 0 {
                let day = weather.forecast[0];
                out[1] = day.max;
                out[2] = day.min;
                out[3] = day.icon as i64;
                out[4] = day.week[0] as i64;
                out[5] = day.week[1] as i64;
                out[6] = day.week[2] as i64;
            }
            weather.forecast_count > 0
        }
        _ => false,
    }
}
