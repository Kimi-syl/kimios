#![no_std]
#![no_main]

//! KimiOS phone shell with a Nothing OS / Material 3 inspired look:
//! monochrome surfaces, dot-matrix clock, red accent, tonal cards, and a
//! Quick Settings shade. Runs entirely in user space.

mod apps;
mod draw;
mod font_data;
mod keyboard;
mod theme;
mod font_store;
mod vt;
mod ai;
mod chat;
mod opencode;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

use draw::Screen;
use libkimi as k;
use crate::theme::{BLACK, SURFACE, SURFACE_HI, SURFACE_MAX, OUTLINE, TEXT, MUTED, RED, RED_DIM, GREEN};

/// User VA of the latency mailbox (0 = unavailable). Stages are written here so
/// the Android frontend can read them out of shared guest RAM.
static INSTR: AtomicU64 = AtomicU64::new(0);
static REQUEST_APP: AtomicU64 = AtomicU64::new(0);

fn appkind_from_u64(v: u64) -> Option<AppKind> {
    Some(match v {
        0 => AppKind::Music, 1 => AppKind::Clock, 2 => AppKind::Settings,
        3 => AppKind::Calculator, 4 => AppKind::Notes, 5 => AppKind::Maps,
        6 => AppKind::Photos, 7 => AppKind::Files, 8 => AppKind::Weather,
        9 => AppKind::Terminal, 10 => AppKind::Browser, 11 => AppKind::Apk,
        12 => AppKind::Dvm, 13 => AppKind::Market, 14 => AppKind::Monitor,
        15 => AppKind::Plugins, 16 => AppKind::AiStudio,
        _ => return None,
    })
}

fn istamp(idx: usize, val: u64) {
    let base = INSTR.load(Ordering::Relaxed);
    if base != 0 {
        unsafe {
            *(base as *mut u64).add(idx) = val;
        }
    }
}

fn ibump_seq() {
    let base = INSTR.load(Ordering::Relaxed);
    if base != 0 {
        unsafe {
            let slot = (base as *mut u64).add(k::IBOX_SEQ);
            *slot += 1;
        }
    }
}


const CLOCK_X: [i64; 60] = [
    0, 107, 213, 316, 416, 512, 602, 685, 761, 828, 887, 935, 974, 1002, 1018,
    1024, 1018, 1002, 974, 935, 887, 828, 761, 685, 602, 512, 416, 316, 213,
    107, 0, -107, -213, -316, -416, -512, -602, -685, -761, -828, -887, -935,
    -974, -1002, -1018, -1024, -1018, -1002, -974, -935, -887, -828, -761,
    -685, -602, -512, -416, -316, -213, -107,
];
const CLOCK_Y: [i64; 60] = [
    -1024, -1018, -1002, -974, -935, -887, -828, -761, -685, -602, -512, -416,
    -316, -213, -107, 0, 107, 213, 316, 416, 512, 602, 685, 761, 828, 887,
    935, 974, 1002, 1018, 1024, 1018, 1002, 974, 935, 887, 828, 761, 685,
    602, 512, 416, 316, 213, 107, 0, -107, -213, -316, -416, -512, -602,
    -685, -761, -828, -887, -935, -974, -1002, -1018,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppKind {
    Music,
    Clock,
    Settings,
    Calculator,
    Notes,
    Maps,
    Photos,
    Files,
    Weather,
    Terminal,
    Browser,
    Apk,
    Dvm,
    Market,
    Monitor,
    Plugins,
    AiStudio,
}

impl AppKind {
    fn index(self) -> usize {
        self as usize
    }

    fn from_index(i: usize) -> Option<AppKind> {
        Some(match i {
            0 => AppKind::Music,
            1 => AppKind::Clock,
            2 => AppKind::Settings,
            3 => AppKind::Calculator,
            4 => AppKind::Notes,
            5 => AppKind::Maps,
            6 => AppKind::Photos,
            7 => AppKind::Files,
            8 => AppKind::Weather,
            9 => AppKind::Terminal,
            10 => AppKind::Browser,
            11 => AppKind::Apk,
            12 => AppKind::Dvm,
            13 => AppKind::Market,
            14 => AppKind::Monitor,
            15 => AppKind::Plugins,
            16 => AppKind::AiStudio,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Empty,
    App(AppKind),
    Folder(u8),
}

#[derive(Clone, Copy)]
struct Folder {
    name: [u8; 24],
    name_len: usize,
    members: [AppKind; 6],
    count: usize,
}

impl Folder {
    const fn new() -> Self {
        Self {
            name: [0; 24],
            name_len: 0,
            members: [AppKind::Music; 6],
            count: 0,
        }
    }

    fn name_str(&self) -> &str {
        str_of(&self.name[..self.name_len])
    }

    fn contains(&self, app: AppKind) -> bool {
        self.members[..self.count].contains(&app)
    }
}

#[derive(Clone, Copy)]
struct Layout {
    grid: [Slot; 9],
    grid2: [Slot; 6],
    dock: [Slot; 4],
    folders: [Folder; 4],
    folder_count: usize,
}

impl Layout {
    fn default() -> Self {
        Self {
            grid: [
                Slot::App(AppKind::Music),
                Slot::App(AppKind::Clock),
                Slot::App(AppKind::Calculator),
                Slot::App(AppKind::Notes),
                Slot::App(AppKind::Maps),
                Slot::App(AppKind::Photos),
                Slot::App(AppKind::Files),
                Slot::App(AppKind::Weather),
                Slot::App(AppKind::Terminal),
            ],
            grid2: [
                Slot::App(AppKind::Browser),
                Slot::App(AppKind::Apk),
                Slot::App(AppKind::AiStudio),
                Slot::App(AppKind::Market),
                Slot::App(AppKind::Monitor),
                Slot::App(AppKind::Plugins),
            ],
            dock: [
                Slot::App(AppKind::Terminal),
                Slot::App(AppKind::Browser),
                Slot::App(AppKind::Files),
                Slot::App(AppKind::Settings),
            ],
            folders: [Folder::new(); 4],
            folder_count: 0,
        }
    }

    fn page(&self, page: usize) -> &[Slot] {
        if page == 1 { &self.grid2 } else { &self.grid }
    }

    fn page_mut(&mut self, page: usize) -> &mut [Slot] {
        if page == 1 { &mut self.grid2 } else { &mut self.grid }
    }

    fn set_slot(&mut self, loc: Loc, slot: Slot) {
        match loc {
            Loc::Grid(i) => self.grid[i] = slot,
            Loc::Grid2(i) => self.grid2[i] = slot,
            Loc::Dock(i) => self.dock[i] = slot,
            Loc::Folder(_, _) => {}
        }
    }

    fn new_folder(&mut self, name: &str, a: AppKind, b: AppKind) -> Option<u8> {
        if self.folder_count >= self.folders.len() {
            return None;
        }
        let idx = self.folder_count;
        let f = &mut self.folders[idx];
        f.count = 2;
        f.members[0] = a;
        f.members[1] = b;
        let nb = name.as_bytes();
        f.name_len = nb.len().min(24);
        f.name[..f.name_len].copy_from_slice(&nb[..f.name_len]);
        self.folder_count += 1;
        Some(idx as u8)
    }

    fn add_to_folder(&mut self, f: usize, app: AppKind) -> bool {
        if f >= self.folder_count {
            return false;
        }
        let folder = &mut self.folders[f];
        if folder.count >= 6 || folder.contains(app) {
            return false;
        }
        folder.members[folder.count] = app;
        folder.count += 1;
        true
    }

    fn remove_from_folder(&mut self, f: usize, member: usize) {
        if f >= self.folder_count || member >= self.folders[f].count {
            return;
        }
        let folder = &mut self.folders[f];
        for i in member..folder.count - 1 {
            folder.members[i] = folder.members[i + 1];
        }
        folder.count -= 1;
    }

    fn first_empty(&self, page: usize) -> Option<usize> {
        self.page(page.min(1)).iter().position(|s| *s == Slot::Empty)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Loc {
    Grid(usize),
    Grid2(usize),
    Dock(usize),
    Folder(usize, usize),
}

impl Loc {
    fn is_grid(self) -> bool {
        matches!(self, Loc::Grid(_) | Loc::Grid2(_))
    }
}

const LAUNCHER_CFG: &str = "/disk/launcher.cfg";

fn save_layout(layout: &Layout) {
    let mut buf = [0u8; 512];
    let mut n = 0usize;
    for (key, slots) in [
        (&b"grid="[..], &layout.grid[..]),
        (b"grid2=", &layout.grid2[..]),
        (b"dock=", &layout.dock[..]),
    ] {
        n = put_bytes(&mut buf, n, key);
        for (i, slot) in slots.iter().enumerate() {
            if i > 0 {
                n = put_bytes(&mut buf, n, b",");
            }
            n = put_slot(&mut buf, n, *slot);
        }
        n = put_bytes(&mut buf, n, b"\n");
    }
    for f in 0..layout.folder_count {
        let folder = &layout.folders[f];
        n = put_bytes(&mut buf, n, b"folder");
        n = put_dec(&mut buf, n, f);
        n = put_bytes(&mut buf, n, b"=");
        n = put_bytes(&mut buf, n, &folder.name[..folder.name_len]);
        n = put_bytes(&mut buf, n, b":");
        for m in 0..folder.count {
            if m > 0 {
                n = put_bytes(&mut buf, n, b",");
            }
            n = put_dec(&mut buf, n, folder.members[m].index());
        }
        n = put_bytes(&mut buf, n, b"\n");
    }
    if let Some(h) = k::create(LAUNCHER_CFG) {
        k::write_file(h, 0, &buf[..n]);
        k::close(h);
    }
}

fn put_bytes(buf: &mut [u8], n: usize, s: &[u8]) -> usize {
    let take = s.len().min(buf.len() - n);
    buf[n..n + take].copy_from_slice(&s[..take]);
    n + take
}

fn put_dec(buf: &mut [u8], n: usize, mut v: usize) -> usize {
    let mut tmp = [0u8; 8];
    let mut i = tmp.len();
    loop {
        i -= 1;
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    put_bytes(buf, n, &tmp[i..])
}

fn put_slot(buf: &mut [u8], n: usize, slot: Slot) -> usize {
    match slot {
        Slot::Empty => put_bytes(buf, n, b"_"),
        Slot::App(app) => put_dec(buf, n, app.index()),
        Slot::Folder(f) => {
            let n = put_bytes(buf, n, b"F");
            put_dec(buf, n, f as usize)
        }
    }
}

fn parse_slot(token: &[u8]) -> Option<Slot> {
    if token == b"_" {
        return Some(Slot::Empty);
    }
    if token.first() == Some(&b'F') {
        let f = parse_dec(&token[1..])?;
        if f < 4 {
            return Some(Slot::Folder(f as u8));
        }
        return None;
    }
    Some(Slot::App(AppKind::from_index(parse_dec(token)?)?))
}

fn parse_dec(token: &[u8]) -> Option<usize> {
    if token.is_empty() {
        return None;
    }
    let mut v = 0usize;
    for &b in token {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (b - b'0') as usize;
        if v > 99 {
            return None;
        }
    }
    Some(v)
}

fn load_layout() -> Layout {
    let mut layout = Layout::default();
    let Some((_, size)) = k::stat(LAUNCHER_CFG) else {
        return layout;
    };
    let Some(h) = k::open(LAUNCHER_CFG) else {
        return layout;
    };
    let mut buf = [0u8; 512];
    let want = (size as usize).min(buf.len());
    let n = k::read_file(h, 0, &mut buf[..want]);
    k::close(h);
    let mut valid = true;
    for line in buf[..n].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Some(eq) = line.iter().position(|&b| b == b'=') else {
            valid = false;
            break;
        };
        let (key, value) = (&line[..eq], &line[eq + 1..]);
        if key.starts_with(b"folder") {
            let Some(f) = parse_dec(&key[6..]) else { valid = false; break };
            if f >= 4 {
                valid = false;
                break;
            }
            let Some(colon) = value.iter().position(|&b| b == b':') else {
                valid = false;
                break;
            };
            let folder = &mut layout.folders[f];
            let name = &value[..colon];
            folder.name_len = name.len().min(24);
            folder.name[..folder.name_len].copy_from_slice(&name[..folder.name_len]);
            folder.count = 0;
            for token in value[colon + 1..].split(|&b| b == b',') {
                if token.is_empty() {
                    continue;
                }
                match parse_dec(token).and_then(AppKind::from_index) {
                    Some(app) if folder.count < 6 => {
                        folder.members[folder.count] = app;
                        folder.count += 1;
                    }
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid {
                break;
            }
            if f >= layout.folder_count {
                layout.folder_count = f + 1;
            }
            continue;
        }
        let slots: &mut [Slot] = match key {
            b"grid" => &mut layout.grid,
            b"grid2" => &mut layout.grid2,
            b"dock" => &mut layout.dock,
            _ => {
                valid = false;
                break;
            }
        };
        let mut i = 0usize;
        for token in value.split(|&b| b == b',') {
            if i >= slots.len() {
                valid = false;
                break;
            }
            match parse_slot(token) {
                Some(slot) => slots[i] = slot,
                None => {
                    valid = false;
                    break;
                }
            }
            i += 1;
        }
        if !valid || i > slots.len() {
            valid = false;
            break;
        }
    }
    if !valid {
        return Layout::default();
    }
    for page in 0..2 {
        for slot in layout.page(page).iter() {
            if let Slot::Folder(f) = slot {
                if *f as usize >= layout.folder_count || layout.folders[*f as usize].count == 0 {
                    return Layout::default();
                }
            }
        }
    }
    layout
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    Lock,
    Home,
    App(AppKind),
    Recents,
}

struct Calculator {
    accumulator: i64,
    current: i64,
    pending: Option<u8>,
    fresh: bool,
}

impl Calculator {
    const fn new() -> Self {
        Self {
            accumulator: 0,
            current: 0,
            pending: None,
            fresh: true,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn apply(&mut self, op: u8) {
        if self.fresh && self.pending.is_some() {
            self.pending = if op == b'=' { None } else { Some(op) };
            return;
        }
        self.accumulator = match self.pending {
            None => self.current,
            Some(pending) => match pending {
                b'+' => self.accumulator.saturating_add(self.current),
                b'-' => self.accumulator.saturating_sub(self.current),
                b'*' => self.accumulator.saturating_mul(self.current),
                b'/' => {
                    if self.current == 0 {
                        0
                    } else {
                        self.accumulator / self.current
                    }
                }
                _ => self.current,
            },
        };
        self.current = self.accumulator;
        self.pending = if op == b'=' { None } else { Some(op) };
        self.fresh = true;
    }

    fn press(&mut self, label: &str) {
        match label {
            "AC" => self.reset(),
            "+/-" => {
                self.current = -self.current;
                self.accumulator = self.current;
                self.fresh = false;
            }
            "<" => {
                if !self.fresh {
                    self.current /= 10;
                }
            }
            "=" => self.apply(b'='),
            "+" | "-" | "*" | "/" => self.apply(label.as_bytes()[0]),
            "." => {}
            _ => {
                if let Some(digit) = label.as_bytes().first() {
                    let digit = digit.wrapping_sub(b'0') as i64;
                    if self.fresh {
                        self.current = digit;
                        self.fresh = false;
                    } else if self.current.unsigned_abs() < 100_000_000 {
                        self.current = self.current * 10 + digit;
                    }
                }
            }
        }
    }
}

struct Settings {
    wifi: bool,
    bluetooth: bool,
    dark: bool,
    dnd: bool,
    torch: bool,
    brightness: i64,
    animations: bool,
}

impl Settings {
    const fn new() -> Self {
        Self {
            wifi: true,
            bluetooth: false,
            dark: true,
            dnd: false,
            torch: false,
            brightness: 70,
            animations: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GMode {
    None,
    Bottom,
    EdgeLeft,
    EdgeRight,
    Drag,
    Consumed,
}

#[derive(Clone, Copy)]
struct Gesture {
    start_x: i64,
    start_y: i64,
    start_ms: u64,
    active: bool,
    mode: GMode,
    long_tried: bool,
}

impl Gesture {
    const fn new() -> Self {
        Self { start_x: 0, start_y: 0, start_ms: 0, active: false, mode: GMode::None, long_tried: false }
    }
}

#[derive(Clone, Copy)]
struct Drag {
    from: Loc,
    slot: Slot,
    hover: Option<Loc>,
    hover_ms: u64,
}

struct Ui {
    scene: Scene,
    touch: (i64, i64),
    press: Option<(i64, i64)>,
    calc: Calculator,
    settings: Settings,
    apps: apps::Apps,
    shift: bool,
    symbols: bool,
    osk: bool,
    shade: bool,
    paint_bg: bool,
    home_page: usize,
    searching: bool,
    search_query: [u8; 128],
    search_len: usize,
    gesture: Gesture,
    layout: Layout,
    edit_mode: bool,
    drag: Option<Drag>,
    folder_view: Option<usize>,
    renaming_folder: Option<usize>,
    recents: [AppKind; 8],
    recent_count: usize,
    recents_scroll: usize,
    scroll_accum: i64,
    fps_x10: u64,
    loop_count: u64,
    fps_window_ms: u64,
    last_jiggle_ms: u64,
    last_monitor_ms: u64,
}

impl Ui {
    fn new(width: i64, height: i64) -> Self {
        Self {
            scene: Scene::Lock,
            touch: (width / 2, height / 2),
            press: None,
            calc: Calculator::new(),
            settings: Settings::new(),
            apps: apps::Apps::new(),
            shift: false,
            symbols: false,
            osk: false,
            shade: false,
            paint_bg: true,
            home_page: 0,
            searching: false,
            search_query: [0u8; 128],
            search_len: 0,
            gesture: Gesture::new(),
            layout: load_layout(),
            edit_mode: false,
            drag: None,
            folder_view: None,
            renaming_folder: None,
            recents: [AppKind::Music; 8],
            recent_count: 0,
            recents_scroll: 0,
            scroll_accum: 0,
            fps_x10: 0,
            loop_count: 0,
            fps_window_ms: 0,
            last_jiggle_ms: 0,
            last_monitor_ms: 0,
        }
    }

    fn push_recent(&mut self, app: AppKind) {
        let mut pos = self.recent_count;
        for i in 0..self.recent_count {
            if self.recents[i] == app {
                pos = i;
                break;
            }
        }
        let end = pos.min(7);
        for i in (1..=end).rev() {
            self.recents[i] = self.recents[i - 1];
        }
        self.recents[0] = app;
        if self.recent_count < 8 {
            self.recent_count += 1;
        }
    }

    fn remove_recent(&mut self, index: usize) {
        if index >= self.recent_count {
            return;
        }
        for i in index..self.recent_count - 1 {
            self.recents[i] = self.recents[i + 1];
        }
        self.recent_count -= 1;
        if self.recents_scroll + 4 > self.recent_count {
            self.recents_scroll = self.recent_count.saturating_sub(4);
        }
    }
}

fn gui_run() -> ! {
    let Some((width, height, stride, _)) = k::fb_info() else {
        k::print("gui: no framebuffer\n");
        k::exit(1);
    };
    let Some(base) = k::fb_back() else {
        k::print("gui: fb_back failed\n");
        k::exit(1);
    };

    let screen = Screen::new(base, width, height, stride);
    theme::load();
    INSTR.store(k::instr_map().unwrap_or(0), Ordering::Relaxed);
    while k::input_poll().is_some() {}
    let mut ui = Ui::new(screen.width(), screen.height());
    k::kplugin::start_services();
    let mut last_minute = u64::MAX;
    let mut last_second = u64::MAX;
    let mut dirty = true;

    loop {
        istamp(k::IBOX_HB, k::cntpct());
        let clock = clock();
        let (minute, second) = (clock[5], clock[6]);
        let now = k::uptime_ms();

        ui.loop_count += 1;
        let req = REQUEST_APP.swap(0, Ordering::Relaxed);
        if req > 0 {
            if let Some(kind) = appkind_from_u64(req - 1) {
                open_app(&screen, &mut ui, kind, clock);
                dirty = true;
            }
        }
        if now.saturating_sub(ui.fps_window_ms) >= 1000 {
            ui.fps_x10 = (ui.loop_count * 10).min(9990);
            ui.loop_count = 0;
            ui.fps_window_ms = now;
        }

        if ui.scene == Scene::App(AppKind::Clock) {
            if second != last_second {
                last_second = second;
                dirty = true;
            }
        } else if ui.scene == Scene::App(AppKind::Music) {
            if second != last_second {
                last_second = second;
                dirty = true;
            }
            if minute != last_minute {
                last_minute = minute;
            }
        } else if ui.scene == Scene::App(AppKind::Monitor) {
            if now.saturating_sub(ui.last_monitor_ms) >= 500 {
                ui.last_monitor_ms = now;
                dirty = true;
            }
        } else if minute != last_minute {
            last_minute = minute;
            dirty = true;
        }

        if ui.scene == Scene::App(AppKind::Browser) && !ui.shade {
            let osk = ui.osk && apps::text_focused(&ui.apps, AppKind::Browser);
            apps::browser_frame(&mut ui.apps, osk);
        }

        if ui.scene == Scene::App(AppKind::Terminal) && !ui.shade {
            if apps::terminal_frame(&mut ui.apps) {
                dirty = true;
            }
        }

        if ui.scene == Scene::App(AppKind::AiStudio) && !ui.shade {
            if apps::ai_frame(&mut ui.apps) {
                dirty = true;
            }
        }

        if ui.press.is_some() && ui.gesture.active && ui.gesture.mode == GMode::None && !ui.gesture.long_tried {
            let dx = ui.touch.0 - ui.gesture.start_x;
            let dy = ui.touch.1 - ui.gesture.start_y;
            if dx * dx + dy * dy < 24 * 24 && now.saturating_sub(ui.gesture.start_ms) >= 400 {
                ui.gesture.long_tried = true;
                if long_press(&screen, &mut ui, clock) {
                    dirty = true;
                }
            }
        }

        if ui.edit_mode && ui.drag.is_none() && now.saturating_sub(ui.last_jiggle_ms) >= 140 {
            ui.last_jiggle_ms = now;
            dirty = true;
        }

        // Animating home (edit-mode jiggle / folder view) moves icons every
        // frame; repaint the wallpaper each frame or old pixels ghost into
        // duplicated logos. (Icon drag uses the fast snapshot path below.)
        if ui.scene == Scene::Home && (ui.edit_mode || ui.folder_view.is_some()) {
            ui.paint_bg = true;
        }
        if ui.drag.is_none() {
            unsafe { DRAG_BG_VALID = false; }
        }

        if dirty {
            if ui.scene == Scene::Home && ui.drag.is_some() {
                drag_render(&screen, &mut ui, clock);
            } else {
                render(&screen, &mut ui, clock);
            }
            dirty = false;
        }

        match k::input_poll() {
            Some(event) => {
                if event.kind == k::EV_KEY
                    && (event.code == k::BTN_LEFT || event.code == k::BTN_TOUCH)
                {
                    istamp(k::IBOX_INPUT, k::cntpct());
                    istamp(k::IBOX_IVAL, event.value as u64);
                    ibump_seq();
                }
                if handle(&screen, &mut ui, event, clock) {
                    dirty = true;
                }
            }
            None => k::sleep_ms(2),
        }
    }
}

/// Long-press while held: launcher edit mode on Home, context menu in Files.
fn long_press(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) -> bool {
    if ui.shade || ui.scene == Scene::Lock {
        return false;
    }
    let point = ui.touch;
    match ui.scene {
        Scene::Home => {
            if ui.searching || ui.drag.is_some() {
                return false;
            }
            if let Some((loc, slot)) = home_slot_hit(screen, ui, point) {
                if slot == Slot::Empty {
                    return false;
                }
                ui.edit_mode = true;
                ui.gesture.mode = GMode::Drag;
                ui.drag = Some(Drag { from: loc, slot, hover: None, hover_ms: 0 });
                render(screen, ui, clock);
                true
            } else {
                false
            }
        }
        Scene::App(app) => {
            let app_point = app_point(screen, point);
            if apps::long_press(&mut ui.apps, app, app_point) {
                ui.gesture.mode = GMode::Consumed;
                ui.osk = false;
                render(screen, ui, clock);
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Bare-metal entry point (EL0 process image loaded by the KimiOS kernel).
#[no_mangle]
#[cfg(target_os = "none")]
pub extern "C" fn _start() -> ! {
    gui_run()
}

/// Library-OS entry point. The embedding host (iOS app / native harness) calls
/// this on a dedicated thread; it never returns. Named differently from `_start`
/// so it does not collide with the host C runtime's ELF entry symbol.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_gui_start() -> ! {
    gui_run()
}

/// Host-only test hook: request the GUI open an app by AppKind discriminant.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_request_app(kind: u64) -> u64 {
    REQUEST_APP.store(kind + 1, Ordering::Relaxed);
    1
}

/// Host-only test hook: load a `.theme` and `.kfont` asset (NUL-terminated path
/// pointers), apply them, and return a success bitmask (bit0 theme, bit1 font).
/// Used by the native harness to verify the downloadable-asset parsers.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_load(theme_path: u64, font_path: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let t = unsafe { cstr(theme_path as *const u8) };
    let f = unsafe { cstr(font_path as *const u8) };
    let ot = theme::load_theme_file(t, "nord");
    let of = font_store::load(f, "pixel-bold");
    if of { theme::apply_font(4); }
    (ot as u64) | ((of as u64) << 1)
}

/// Host-only test hook: verify the OSK symbol layer (63 = all checks pass).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_osk() -> u64 {
    keyboard::self_test()
}

/// Host-only test hook: fetch+cache the kpkg repo (1 = success).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_kpkg() -> u64 {
    k::kpkg::update_quiet("") as u64
}

/// Host-only test hook: install a repo asset to dest (1 = success).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_install(name: u64, dest: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let nm = unsafe { cstr(name as *const u8) };
    let ds = unsafe { cstr(dest as *const u8) };
    k::kpkg::install_to(nm, ds) as u64
}

/// Host-only test hook: validate + install a local .kpkg/.kfont/.ktheme.
/// Returns (ok bit0) | (kind << 1) where kind is 0 theme, 1 font, 2 app.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_import(path: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let p = unsafe { cstr(path as *const u8) };
    let r = k::kpkg::import_local(p);
    (r.ok as u64) | ((r.kind as u64) << 1)
}

/// Host-only test hook: auto-import an Alpine package as a plugin (1 = success).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_import_alpine(pkg: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let p = unsafe { cstr(pkg as *const u8) };
    k::kimport::import_alpine(p) as u64
}

/// Host-only test hook: build the Marketplace store with a source filter
/// (0 all, 1 installed, 2 alpine, 3 npm) + search query; returns the row count.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_store_count(filter: u64, query: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let q = unsafe { cstr(query as *const u8) };
    apps::test_store_count(filter as u8, q)
}

/// Host-only test hook: write "name|name|..." of the first store rows to `out`.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_store_names(filter: u64, query: u64, out: u64, cap: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let q = unsafe { cstr(query as *const u8) };
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, cap as usize) };
    apps::test_store_names(filter as u8, q, buf) as u64
}

/// Host-only test hook: run a store row by (name, src); copies captured output
/// into `out`, returns run_len + 1 (0 = row not found).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_store_run(name: u64, src: u64, out: u64, cap: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let nm = unsafe { cstr(name as *const u8) };
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, cap as usize) };
    apps::test_store_run(nm, src as u8, buf)
}

/// Host-only VT parser test: feed a NUL-terminated escape-script, render the
/// plain-text grid into out, return (cy<<8)|cx.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_vt(script: u64, out: u64, cap: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let s = unsafe { cstr(script as *const u8) };
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, cap as usize) };
    apps::test_vt_render(s, buf)
}

/// Device test: interactive PTY shell round-trip; copies raw stream into out.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_term(out: u64, cap: u64) -> u64 {
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, cap as usize) };
    apps::test_term(buf)
}

/// Host-only AI client test: build a sample chat request into out; returns len.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_ai_request(out: u64, cap: u64) -> u64 {
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, cap as usize) };
    ai::test_request(buf) as u64
}

/// Host-only AI client test: parse an SSE chunk; decoded text into out, returns
/// (done<<32)|len.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_ai_sse(chunk: u64, out: u64, cap: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let s = unsafe { cstr(chunk as *const u8) };
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, cap as usize) };
    ai::test_sse(s, buf)
}

/// Host-only AI end-to-end: POST prompt to llama-server, stream reply into out.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_ai_chat(prompt: u64, out: u64, cap: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let s = unsafe { cstr(prompt as *const u8) };
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, cap as usize) };
    ai::test_chat(s, buf)
}

/// Host-only OpenCode client test: drive a live `opencode serve` (Basic auth).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_opencode(pw: u64, prompt: u64, out: u64, cap: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let password = unsafe { cstr(pw as *const u8) };
    let pr = unsafe { cstr(prompt as *const u8) };
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, cap as usize) };
    opencode::test_opencode(password, pr, buf)
}
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_plugin_list(out: u64) -> u64 {
    let mut infos = [k::kplugin::PluginInfo {
        id: [0; 48], id_len: 0, name: [0; 48], name_len: 0,
        version: [0; 16], version_len: 0, ptype: 0, runtime: 0, enabled: false,
    }; 32];
    let n = k::kplugin::list(&mut infos);
    let buf = unsafe { core::slice::from_raw_parts_mut(out as *mut u8, 4096) };
    let mut used = 0usize;
    for i in 0..n {
        let p = &infos[i];
        let mut line = [0u8; 64];
        let mut ln = 0usize;
        let id = p.id_str().as_bytes();
        let take = id.len().min(48);
        line[..take].copy_from_slice(&id[..take]);
        ln += take;
        line[ln] = b' '; ln += 1;
        line[ln] = b'0' + p.ptype; ln += 1;
        line[ln] = b' '; ln += 1;
        line[ln] = if p.enabled { b'1' } else { b'0' }; ln += 1;
        line[ln] = b'\n'; ln += 1;
        let copy = ln.min(buf.len() - used);
        buf[used..used + copy].copy_from_slice(&line[..copy]);
        used += copy;
    }
    n as u64
}

/// Host-only test hook: run an app plugin by id. Returns exit_code + 1, or 0
/// when the run is unavailable.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_plugin_run(id: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let id = unsafe { cstr(id as *const u8) };
    match k::kplugin::run(id) {
        Some(code) => code + 1,
        None => 0,
    }
}

/// Host-only test hook: Plugin Manager run with console-output capture.
/// Returns run_len + 1 (output view), 0 (exit-code path), MAX (unknown id).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_plugin_out(id: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let id = unsafe { cstr(id as *const u8) };
    apps::test_plugin_output(id)
}

/// Host-only test hook: enable/disable a plugin (1 = success).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_plugin_enable(id: u64, on: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let id = unsafe { cstr(id as *const u8) };
    k::kplugin::set_enabled(id, on != 0) as u64
}

/// Host-only test hook: uninstall a plugin (1 = success).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_plugin_uninstall(id: u64) -> u64 {
    unsafe fn cstr(p: *const u8) -> &'static str {
        let mut n = 0;
        while *p.add(n) != 0 { n += 1; }
        core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("")
    }
    let id = unsafe { cstr(id as *const u8) };
    k::kplugin::uninstall(id) as u64
}

/// Host-only test hook: Plugin API server port (0 = unavailable).
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_plugin_api() -> u64 {
    k::kplugin::api_port() as u64
}

/// Host-only test hook: start enabled service plugins; returns 1.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_plugin_services() -> u64 {
    k::kplugin::start_services();
    1
}

/// Host-only test hook: parse canned weather JSON (mode 1 HKO rhrread,
/// 2 Open-Meteo current, 3 HKO fnd); results written to out[0..8].
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_weather(json: u64, len: u64, mode: u64, out: u64) -> u64 {
    let data = unsafe { core::slice::from_raw_parts(json as *const u8, len as usize) };
    let out = unsafe { core::slice::from_raw_parts_mut(out as *mut i64, 8) };
    apps::weather_parse_test(data, mode, out) as u64
}

/// Host-only test hook: bounds-checked RGBA→XRGB blit; returns rows written.
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_blit(
    src: u64, sw: u64, sh: u64,
    dst: u64, stride: u64,
    dx: u64, dy: u64, maxw: u64, maxh: u64,
) -> u64 {
    apps::blit_rgba_to_xrgb(
        src as *const u8, sw as i64, sh as i64,
        dst as *mut u32, stride as i64,
        dx as i64, dy as i64, maxw as i64, maxh as i64,
    ) as u64
}

/// Host-only test hook: drive the real handle() with packed input events
/// (kind<<48 | code<<32 | value) against a seeded app list. Returns the app's
/// scroll position; out[0..4] = [scene, state, osk, scroll_accum].
#[no_mangle]
#[cfg(not(target_os = "none"))]
pub extern "C" fn kimi_test_scroll(app: u64, events: u64, n: u64, out: u64) -> u64 {
    static mut TEST_FB: [u32; 1080 * 1920] = [0; 1080 * 1920];
    let Some(kind) = AppKind::from_index(app as usize) else { return u64::MAX };
    let screen = Screen::new(core::ptr::addr_of_mut!(TEST_FB) as u64, 1080, 1920, 1080 * 4);
    let mut ui = Ui::new(screen.width(), screen.height());
    ui.scene = Scene::App(kind);
    apps::test_seed(&mut ui.apps, kind);
    let clock = [0, 2026, 1, 1, 0, 0, 0];
    let packed = unsafe { core::slice::from_raw_parts(events as *const u64, n as usize) };
    for &raw in packed {
        let event = k::InputEvent {
            kind: (raw >> 48) as u16,
            code: ((raw >> 32) & 0xffff) as u16,
            value: raw as u32,
        };
        handle(&screen, &mut ui, event, clock);
    }
    let scene = match ui.scene {
        Scene::Lock => 0,
        Scene::Home => 1,
        Scene::App(_) => 2,
        Scene::Recents => 3,
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out as *mut u64, 4) };
    out[0] = scene;
    out[1] = apps::test_state(&ui.apps, kind);
    out[2] = ui.osk as u64;
    out[3] = ui.scroll_accum as u64;
    apps::test_scroll(&ui.apps, kind)
}

fn handle(screen: &Screen, ui: &mut Ui, event: k::InputEvent, clock: [u64; 7]) -> bool {
    let (width, height) = (screen.width(), screen.height());

    match (event.kind, event.code) {
        (k::EV_ABS, k::ABS_X) => {
            ui.touch.0 = scale(event.value, width);
            if drag_move(screen, ui, clock) {
                return true;
            }
            browser_drag_move(screen, ui);
            false
        }
        (k::EV_ABS, k::ABS_Y) => {
            let new_y = scale(event.value, height);
            let dy = new_y - ui.touch.1;
            let mut scrolled = false;
            if let Scene::App(app) = ui.scene {
                if ui.press.is_some()
                    && ui.gesture.mode == GMode::None
                    && !ui.shade
                    && dy.abs() >= 2
                {
                    let dx = ui.touch.0 - ui.gesture.start_x;
                    let dy_total = new_y - ui.gesture.start_y;
                    if dy_total.abs() >= dx.abs() && dx * dx + dy_total * dy_total >= 24 * 24 {
                        let point = app_point(screen, (ui.touch.0, new_y));
                        let osk_now = ui.osk && apps::text_focused(&ui.apps, app);
                        if !(osk_now && point.1 >= keyboard::TOP)
                            && apps::scroll_drag_hit(&ui.apps, app, point, osk_now)
                        {
                            let dy_app = dy * 800 / screen.height();
                            if apps::drag_scroll(&mut ui.apps, app, dy_app, &mut ui.scroll_accum) {
                                ui.gesture.long_tried = true;
                                scrolled = true;
                            }
                        }
                    }
                }
            }
            ui.touch.1 = new_y;
            if drag_move(screen, ui, clock) {
                return true;
            }
            browser_drag_move(screen, ui);
            scrolled
        }
        (k::EV_KEY, k::KEY_ESC) if event.value != 0 => {
            k::exit(0);
        }
        (k::EV_KEY, 42) | (k::EV_KEY, 54) => {
            ui.shift = event.value != 0;
            false
        }
        (k::EV_KEY, code)
            if event.value != 0 && code != k::BTN_LEFT && code != k::BTN_TOUCH =>
        {
            if ui.scene == Scene::Lock {
                if code == k::KEY_ENTER || code == k::KEY_SPACE || code == k::KEY_UP {
                    unlock(screen, ui, clock);
                    return true;
                }
                return false;
            }
            if ui.scene == Scene::Home && ui.searching {
                if search_input(ui, code, ui.shift) {
                    if ui.shift && code != 14 {
                        ui.shift = false;
                    }
                    render(screen, ui, clock);
                    return true;
                }
                return false;
            }
            if let Scene::App(app) = ui.scene {
                if apps::key(&mut ui.apps, app, code, ui.shift) {
                    render(screen, ui, clock);
                    return true;
                }
            }
            false
        }
        (k::EV_KEY, k::BTN_LEFT) | (k::EV_KEY, k::BTN_TOUCH) => {
            if event.value == 0 {
                if ui.scene == Scene::App(AppKind::Browser) && !ui.shade {
                    let point = app_point(screen, ui.touch);
                    apps::browser_touch_up(&mut ui.apps, point);
                }
                let released = ui.press.take();
                let gesture = ui.gesture;
                ui.gesture.active = false;
                let now = k::uptime_ms();
                let mut handled = false;
                if released.is_some() && ui.scene != Scene::Lock {
                    let dx = ui.touch.0 - gesture.start_x;
                    let dy = ui.touch.1 - gesture.start_y;
                    let dt = now.saturating_sub(gesture.start_ms);
                    if ui.drag.is_some() {
                        drop_drag(screen, ui, clock);
                        handled = true;
                    } else if gesture.mode == GMode::Consumed {
                        handled = true;
                    } else if ui.scene == Scene::Recents {
                        handled = recents_gesture(screen, ui, clock, gesture, dx, dy);
                    } else if gesture.mode == GMode::Bottom
                        && dy < -80
                        && dy.abs() > dx.abs()
                    {
                        match ui.scene {
                            Scene::App(_) => {
                                if dt >= 300 {
                                    open_recents(screen, ui, clock);
                                } else {
                                    close_app(screen, ui, clock);
                                }
                            }
                            Scene::Home => open_recents(screen, ui, clock),
                            _ => {}
                        }
                        handled = true;
                    } else if dx.abs() > 80
                        && dx.abs() > dy.abs() * 2
                        && ((gesture.mode == GMode::EdgeLeft && dx > 0)
                            || (gesture.mode == GMode::EdgeRight && dx < 0))
                    {
                        if let Scene::App(app) = ui.scene {
                            if ui.osk {
                                ui.osk = false;
                                ui.shift = false;
                                ui.symbols = false;
                                render(screen, ui, clock);
                            } else if app == AppKind::Browser && apps::browser_can_back(&ui.apps) {
                                apps::browser_web_back(&mut ui.apps);
                                render(screen, ui, clock);
                            } else if apps::nav_back(&mut ui.apps, app) {
                                ui.osk = apps::text_focused(&ui.apps, app);
                                render(screen, ui, clock);
                            } else {
                                close_app(screen, ui, clock);
                            }
                            handled = true;
                        }
                    }
                }
                render(screen, ui, clock);
                if !handled {
                    if let Some(start) = released {
                        if ui.scene == Scene::Lock {
                            unlock(screen, ui, clock);
                        } else {
                            let dx = ui.touch.0 - start.0;
                            let dy = ui.touch.1 - start.1;
                            if dx * dx + dy * dy < 24 * 24 {
                                tapped(screen, ui, ui.touch, clock);
                            } else if ui.scene == Scene::Home && dx.abs() > 60 && dx.abs() > dy.abs() * 2 {
                                let next = if dx < 0 { (ui.home_page + 1).min(2) } else { ui.home_page.saturating_sub(1) };
                                if ui.searching {
                                    ui.searching = false;
                                    ui.osk = false;
                                    ui.shift = false;
                                    ui.symbols = false;
                                    ui.paint_bg = true;
                                    render(screen, ui, clock);
                                } else if ui.home_page != next && ui.drag.is_none() {
                                    ui.home_page = next;
                                    if next == 2 {
                                        apps::launcher_refresh(&mut ui.apps);
                                        ui.edit_mode = false;
                                        ui.folder_view = None;
                                    }
                                    ui.paint_bg = true;
                                    render(screen, ui, clock);
                                }
                            }
                        }
                    }
                }
                true
            } else {
                istamp(k::IBOX_HANDLER, k::cntpct());
                ui.press = Some(ui.touch);
                ui.scroll_accum = 0;
                ui.gesture = Gesture {
                    start_x: ui.touch.0,
                    start_y: ui.touch.1,
                    start_ms: k::uptime_ms(),
                    active: true,
                    mode: GMode::None,
                    long_tried: false,
                };
                if ui.scene != Scene::Lock && !ui.shade {
                    let bottom_band = height - 60;
                    if ui.touch.1 >= bottom_band {
                        ui.gesture.mode = GMode::Bottom;
                    } else if matches!(ui.scene, Scene::App(_) | Scene::Recents) {
                        if ui.touch.0 <= 20 {
                            ui.gesture.mode = GMode::EdgeLeft;
                        } else if ui.touch.0 >= width - 20 {
                            ui.gesture.mode = GMode::EdgeRight;
                        }
                    }
                }
                if ui.scene == Scene::Home && ui.edit_mode && !ui.searching {
                    if let Some((loc, slot)) = home_slot_hit(screen, ui, ui.touch) {
                        let allowed = match loc {
                            Loc::Folder(_, _) => ui.folder_view.is_some(),
                            _ => ui.folder_view.is_none(),
                        };
                        if allowed && slot != Slot::Empty {
                            ui.gesture.mode = GMode::Drag;
                            ui.drag = Some(Drag { from: loc, slot, hover: None, hover_ms: 0 });
                        }
                    }
                }
                if ui.scene == Scene::App(AppKind::Browser) && !ui.shade {
                    let point = app_point(screen, ui.touch);
                    let osk_now = ui.osk && apps::text_focused(&ui.apps, AppKind::Browser);
                    if !(osk_now && point.1 >= keyboard::TOP)
                        && apps::browser_touch_down(&mut ui.apps, point)
                    {
                        return false;
                    }
                }
                // Instant press feedback: draw just the touch ring over the
                // existing frame and present only its small bounding box,
                // instead of redrawing the whole scene (keeps clicks snappy).
                press_feedback(screen, ui);
                false
            }
        }
        _ => false,
    }
}

fn drag_move(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) -> bool {
    if ui.drag.is_none() || ui.scene != Scene::Home {
        return false;
    }
    let from = ui.drag.map(|d| d.from);
    let target = home_slot_hit(screen, ui, ui.touch)
        .map(|(loc, _)| loc)
        .filter(|loc| Some(*loc) != from);
    let hover = ui.drag.and_then(|d| d.hover);
    if hover != target {
        if let Some(d) = ui.drag.as_mut() {
            d.hover = target;
            d.hover_ms = k::uptime_ms();
        }
    }
    render(screen, ui, clock);
    true
}

fn drop_drag(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    let Some(drag) = ui.drag.take() else { return };
    let now = k::uptime_ms();
    let target = home_slot_hit(screen, ui, ui.touch);
    let mut changed = false;
    let held = drag.hover.is_some() && now.saturating_sub(drag.hover_ms) >= 400;
    match drag.from {
        Loc::Folder(f, m) => {
            if let Slot::App(app) = drag.slot {
                let mut placed = false;
                match target {
                    Some((loc, Slot::Empty)) if loc.is_grid() => {
                        ui.layout.set_slot(loc, Slot::App(app));
                        placed = true;
                    }
                    Some((loc, Slot::App(other))) if loc.is_grid() => {
                        if let Some(nf) = ui.layout.new_folder("Folder", other, app) {
                            ui.layout.set_slot(loc, Slot::Folder(nf));
                            placed = true;
                        }
                    }
                    Some((_, Slot::Folder(f2))) => {
                        if ui.layout.add_to_folder(f2 as usize, app) {
                            placed = true;
                        }
                    }
                    None => {
                        if let Some(i) = ui.layout.first_empty(ui.home_page) {
                            let loc = if ui.home_page == 1 { Loc::Grid2(i) } else { Loc::Grid(i) };
                            ui.layout.set_slot(loc, Slot::App(app));
                            placed = true;
                        }
                    }
                    _ => {}
                }
                if placed {
                    ui.layout.remove_from_folder(f, m);
                    changed = true;
                }
            }
        }
        from => {
            match target {
                Some((loc, target_slot)) if loc != from => {
                    match (drag.slot, target_slot) {
                        (Slot::App(a), Slot::App(b)) if loc.is_grid() && held => {
                            if let Some(nf) = ui.layout.new_folder("Folder", b, a) {
                                ui.layout.set_slot(loc, Slot::Folder(nf));
                                ui.layout.set_slot(from, Slot::Empty);
                                changed = true;
                            }
                        }
                        (Slot::App(a), Slot::Folder(f)) => {
                            if ui.layout.add_to_folder(f as usize, a) {
                                ui.layout.set_slot(from, Slot::Empty);
                                changed = true;
                            }
                        }
                        (Slot::Folder(f1), Slot::Folder(f2)) if f1 != f2 => {
                            let src = ui.layout.folders[f1 as usize];
                            let mut merged = false;
                            for i in 0..src.count {
                                if ui.layout.add_to_folder(f2 as usize, src.members[i]) {
                                    merged = true;
                                }
                            }
                            if merged {
                                ui.layout.folders[f1 as usize].count = 0;
                                ui.layout.set_slot(from, Slot::Empty);
                                changed = true;
                            }
                        }
                        _ => {
                            ui.layout.set_slot(loc, drag.slot);
                            ui.layout.set_slot(from, target_slot);
                            changed = true;
                        }
                    }
                }
                None => {
                    if matches!(from, Loc::Dock(_)) {
                        if let Some(i) = ui.layout.first_empty(ui.home_page) {
                            let loc = if ui.home_page == 1 { Loc::Grid2(i) } else { Loc::Grid(i) };
                            ui.layout.set_slot(loc, drag.slot);
                            ui.layout.set_slot(from, Slot::Empty);
                            changed = true;
                        }
                    } else if from.is_grid() && ui.touch.1 >= screen.height() - 130 {
                        let index = ((ui.touch.0 - 20) / 109).clamp(0, 3) as usize;
                        let old = ui.layout.dock[index];
                        ui.layout.dock[index] = drag.slot;
                        ui.layout.set_slot(from, old);
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }
    if changed {
        for page in 0..2 {
            for i in 0..ui.layout.page(page).len() {
                if let Slot::Folder(f) = ui.layout.page(page)[i] {
                    if ui.layout.folders[f as usize].count == 0 {
                        ui.layout.page_mut(page)[i] = Slot::Empty;
                    }
                }
            }
        }
        for i in 0..4 {
            if let Slot::Folder(f) = ui.layout.dock[i] {
                if ui.layout.folders[f as usize].count == 0 {
                    ui.layout.dock[i] = Slot::Empty;
                }
            }
        }
        if let Some(f) = ui.folder_view {
            if ui.layout.folders[f].count == 0 {
                ui.folder_view = None;
            }
        }
        save_layout(&ui.layout);
    }
    render(screen, ui, clock);
}

fn recents_gesture(
    screen: &Screen,
    ui: &mut Ui,
    clock: [u64; 7],
    gesture: Gesture,
    dx: i64,
    dy: i64,
) -> bool {
    if gesture.mode == GMode::Bottom && dy < -80 && dy.abs() > dx.abs() {
        go_home(screen, ui, clock);
        return true;
    }
    if matches!(gesture.mode, GMode::EdgeLeft | GMode::EdgeRight) && dx.abs() > 80 {
        go_home(screen, ui, clock);
        return true;
    }
    if dy < -80 || dx.abs() > 120 {
        if let Some(index) = recents_card_at(screen, ui, (gesture.start_x, gesture.start_y)) {
            ui.remove_recent(index);
            ui.paint_bg = true;
            render(screen, ui, clock);
            return true;
        }
    }
    false
}

fn app_point(screen: &Screen, point: (i64, i64)) -> (i64, i64) {
    (point.0, point.1 * 800 / screen.height())
}

fn browser_drag_move(screen: &Screen, ui: &mut Ui) {
    if ui.scene != Scene::App(AppKind::Browser) || ui.shade || ui.press.is_none() {
        return;
    }
    let point = app_point(screen, ui.touch);
    apps::browser_touch_move(&mut ui.apps, point);
}

fn press_feedback(screen: &Screen, ui: &Ui) {
    if let Some(point) = ui.press {
        screen.reset_dirty();
        screen.ring(point.0, point.1, 18, 3, 0x00a0_a8b8);
        screen.circle(point.0, point.1, 3, TEXT());
        istamp(k::IBOX_DRAWN, k::cntpct());
        present_dirty(screen);
    }
}

fn tapped(screen: &Screen, ui: &mut Ui, point: (i64, i64), clock: [u64; 7]) {
    let logical_height = screen.height();
    let app_point = (point.0, point.1 * 800 / logical_height);
    if ui.shade {
        shade_tap(screen, ui, app_point, clock);
        return;
    }
    if point.1 < 30 {
        toggle_shade(screen, ui, clock, true);
        return;
    }

    match ui.scene {
        Scene::Lock => unlock(screen, ui, clock),
        Scene::Recents => {
            if recents_clear_hit(screen, point) {
                ui.recent_count = 0;
                ui.recents_scroll = 0;
                ui.paint_bg = true;
                render(screen, ui, clock);
                return;
            }
            if let Some(dir) = recents_scroll_hit(screen, point) {
                if dir < 0 {
                    ui.recents_scroll = ui.recents_scroll.saturating_sub(1);
                } else if ui.recents_scroll + 4 < ui.recent_count {
                    ui.recents_scroll += 1;
                }
                ui.paint_bg = true;
                render(screen, ui, clock);
                return;
            }
            if let Some(index) = recents_card_at(screen, ui, point) {
                let app = ui.recents[index];
                open_app(screen, ui, app, clock);
                return;
            }
            go_home(screen, ui, clock);
        }
        Scene::Home => {
            if ui.searching && ui.osk {
                if let Some(key) = keyboard::hit(app_point, ui.symbols) {
                    osk_key_home(screen, ui, key, clock);
                    return;
                }
                if app_point.1 >= keyboard::TOP && app_point.1 <= 770 {
                    return;
                }
            }
            if ui.edit_mode && !ui.searching && edit_done_hit(screen, point) {
                ui.edit_mode = false;
                ui.folder_view = None;
                save_layout(&ui.layout);
                ui.paint_bg = true;
                render(screen, ui, clock);
                return;
            }
            if search_pill_hit(screen, point) {
                if ui.searching && search_go_hit(screen, point) {
                    search_go(ui);
                } else {
                    ui.searching = true;
                    ui.osk = true;
                }
                render(screen, ui, clock);
                return;
            }
            if ui.searching {
                ui.searching = false;
                ui.osk = false;
                ui.shift = false;
                ui.symbols = false;
                ui.renaming_folder = None;
                render(screen, ui, clock);
                return;
            }
            if ui.edit_mode {
                if edit_done_hit(screen, point) {
                    ui.edit_mode = false;
                    ui.folder_view = None;
                    save_layout(&ui.layout);
                    ui.paint_bg = true;
                    render(screen, ui, clock);
                } else if let Some(f) = ui.folder_view {
                    if folder_rename_hit(screen, point) {
                        start_folder_rename(ui, f);
                        render(screen, ui, clock);
                    } else if !folder_panel_hit(screen, point) {
                        ui.folder_view = None;
                        render(screen, ui, clock);
                    }
                }
                return;
            }
            if let Some(f) = ui.folder_view {
                if let Some(m) = folder_member_hit(screen, ui, f, point) {
                    let app = ui.layout.folders[f].members[m];
                    ui.folder_view = None;
                    open_app(screen, ui, app, clock);
                } else if folder_rename_hit(screen, point) {
                    start_folder_rename(ui, f);
                    render(screen, ui, clock);
                } else if !folder_panel_hit(screen, point) {
                    ui.folder_view = None;
                    render(screen, ui, clock);
                }
                return;
            }
            if let Some(page) = home_page_hit(screen, point) {
                if ui.home_page != page {
                    ui.home_page = page;
                    if page == 2 {
                        apps::launcher_refresh(&mut ui.apps);
                        ui.edit_mode = false;
                        ui.folder_view = None;
                    }
                    ui.paint_bg = true;
                    render(screen, ui, clock);
                }
            } else if music_hit(screen, point) {
                apps::home_music_toggle(&mut ui.apps);
                render(screen, ui, clock);
            } else if ui.home_page == 2 {
                if let Some(tile) = plugin_home_hit(screen, ui, point) {
                    if tile == 0 {
                        open_app(screen, ui, AppKind::Plugins, clock);
                    } else {
                        apps::launcher_run(&mut ui.apps, tile - 1);
                        render(screen, ui, clock);
                    }
                }
            } else if let Some((_, slot)) = home_slot_hit(screen, ui, point) {
                match slot {
                    Slot::App(app) => open_app(screen, ui, app, clock),
                    Slot::Folder(f) => {
                        ui.folder_view = Some(f as usize);
                        ui.paint_bg = true;
                        render(screen, ui, clock);
                    }
                    Slot::Empty => {}
                }
            }
        }
        Scene::App(app) => {
            let osk_now = ui.osk && apps::text_focused(&ui.apps, app);
            if osk_now {
                if let Some(key) = keyboard::hit(app_point, ui.symbols) {
                    osk_key(screen, ui, app, key, clock);
                    return;
                }
                // Swallow taps in the keyboard panel (gaps included) so they
                // never reach app logic; the gesture bar below still closes.
                if app_point.1 >= keyboard::TOP && app_point.1 <= 770 {
                    return;
                }
            }
            if back_hit(screen, app_point) {
                if apps::nav_back(&mut ui.apps, app) {
                    ui.osk = apps::text_focused(&ui.apps, app);
                    render(screen, ui, clock);
                } else {
                    close_app(screen, ui, clock);
                }
                return;
            }
            if apps::back(&mut ui.apps, app) {
                render(screen, ui, clock);
                return;
            }
            let focused_before = apps::text_focused(&ui.apps, app);
            match app {
                AppKind::Calculator => {
                    if let Some(index) = calculator_hit(app_point) {
                        ui.calc.press(CALCULATOR_LABELS[index]);
                    }
                }
                AppKind::Settings => {
                    if let Some(row) = settings_hit(screen, app_point) {
                        toggle_settings(&mut ui.settings, row);
                    }
                }
                _ => apps::tap(screen, &mut ui.apps, app, app_point, osk_now, clock),
            }
            if apps::text_focused(&ui.apps, app)
                && (!focused_before || apps::field_hit(&ui.apps, app, app_point, osk_now))
            {
                ui.osk = true;
            }
            render(screen, ui, clock);
        }
    }
}

/// Dispatch an on-screen keyboard key through the same path as hardware
/// keys. Shift is one-shot: the next character consumes it.
fn osk_key(screen: &Screen, ui: &mut Ui, app: AppKind, key: keyboard::Key, clock: [u64; 7]) {
    let trace = match key {
        keyboard::Key::Hide => {
            ui.osk = false;
            ui.shift = false;
            ui.symbols = false;
            901
        }
        keyboard::Key::Shift => {
            ui.shift = !ui.shift;
            900
        }
        keyboard::Key::Symbols => {
            ui.symbols = true;
            ui.shift = false;
            902
        }
        keyboard::Key::Letters => {
            ui.symbols = false;
            ui.shift = false;
            903
        }
        keyboard::Key::Sym(code, sh) => {
            apps::key(&mut ui.apps, app, code, sh);
            code as u64
        }
        keyboard::Key::Char(code) => {
            apps::key(&mut ui.apps, app, code, ui.shift);
            if ui.shift && code != 14 {
                ui.shift = false;
            }
            code as u64
        }
    };
    k::print("osk: code ");
    k::print_u64(trace);
    k::print("\n");
    render(screen, ui, clock);
}

fn osk_key_home(screen: &Screen, ui: &mut Ui, key: keyboard::Key, clock: [u64; 7]) {
    match key {
        keyboard::Key::Hide => {
            ui.osk = false;
            ui.shift = false;
            ui.symbols = false;
        }
        keyboard::Key::Shift => {
            ui.shift = !ui.shift;
        }
        keyboard::Key::Symbols => {
            ui.symbols = true;
            ui.shift = false;
        }
        keyboard::Key::Letters => {
            ui.symbols = false;
            ui.shift = false;
        }
        keyboard::Key::Sym(code, sh) => {
            search_input(ui, code, sh);
        }
        keyboard::Key::Char(code) => {
            search_input(ui, code, ui.shift);
            if ui.shift && code != 14 {
                ui.shift = false;
            }
        }
    }
    render(screen, ui, clock);
}

fn search_input(ui: &mut Ui, code: u16, shift: bool) -> bool {
    match apps::key_char(code, shift) {
        Some((0, true)) => {
            ui.search_len = ui.search_len.saturating_sub(1);
            true
        }
        Some((b'\n', _)) => {
            search_go(ui);
            true
        }
        Some((byte, _)) => {
            if ui.search_len < ui.search_query.len() {
                ui.search_query[ui.search_len] = byte;
                ui.search_len += 1;
            }
            true
        }
        None => false,
    }
}

fn start_folder_rename(ui: &mut Ui, f: usize) {
    ui.renaming_folder = Some(f);
    ui.searching = true;
    ui.osk = true;
    ui.search_len = 0;
    let name = &ui.layout.folders[f].name[..ui.layout.folders[f].name_len];
    let len = name.len().min(ui.search_query.len());
    ui.search_query[..len].copy_from_slice(&name[..len]);
    ui.search_len = len;
}

fn search_go(ui: &mut Ui) {
    if let Some(f) = ui.renaming_folder.take() {
        if ui.search_len > 0 && f < ui.layout.folder_count {
            let len = ui.search_len.min(24);
            ui.layout.folders[f].name[..len].copy_from_slice(&ui.search_query[..len]);
            ui.layout.folders[f].name_len = len;
            save_layout(&ui.layout);
        }
        ui.searching = false;
        ui.osk = false;
        ui.shift = false;
        ui.symbols = false;
        ui.search_len = 0;
        return;
    }
    if ui.search_len == 0 {
        return;
    }
    let mut url = [0u8; 448];
    let prefix = b"https://duckduckgo.com/?q=";
    url[..prefix.len()].copy_from_slice(prefix);
    let mut used = prefix.len();
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in &ui.search_query[..ui.search_len] {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            if used + 1 > url.len() {
                break;
            }
            url[used] = byte;
            used += 1;
        } else if byte == b' ' {
            if used + 1 > url.len() {
                break;
            }
            url[used] = b'+';
            used += 1;
        } else {
            if used + 3 > url.len() {
                break;
            }
            url[used] = b'%';
            url[used + 1] = HEX[(byte >> 4) as usize];
            url[used + 2] = HEX[(byte & 15) as usize];
            used += 3;
        }
    }
    let _ = k::open_url(str_of(&url[..used]));
    ui.searching = false;
    ui.osk = false;
    ui.shift = false;
    ui.symbols = false;
}

fn search_pill_hit(screen: &Screen, point: (i64, i64)) -> bool {
    if screen.height() < 720 {
        return false;
    }
    point.0 >= 24
        && point.0 <= screen.width() - 24
        && point.1 >= screen.sy(142)
        && point.1 <= screen.sy(196)
}

fn search_go_hit(screen: &Screen, point: (i64, i64)) -> bool {
    point.0 >= screen.width() - 104
        && point.0 <= screen.width() - 28
        && point.1 >= screen.sy(148)
        && point.1 <= screen.sy(190)
}

fn shade_tap(screen: &Screen, ui: &mut Ui, point: (i64, i64), clock: [u64; 7]) {
    let height = 800;
    if point.1 > 610 {
        toggle_shade(screen, ui, clock, false);
        return;
    }
    if let Some(toggle) = shade_toggle_hit(point) {
        match toggle {
            0 => ui.settings.wifi = !ui.settings.wifi,
            1 => ui.settings.bluetooth = !ui.settings.bluetooth,
            2 => ui.settings.dnd = !ui.settings.dnd,
            _ => ui.settings.torch = !ui.settings.torch,
        }
        render(screen, ui, clock);
        return;
    }
    if point.1 >= 318 && point.1 <= 366 {
        let level = ((point.0 - 24).clamp(0, 432) * 100 / 432).clamp(8, 100);
        ui.settings.brightness = level;
        render(screen, ui, clock);
        return;
    }
    if point.1 <= 64 && point.0 > screen.width() - 74 {
        // Settings shortcut in the shade header.
        toggle_shade(screen, ui, clock, false);
        open_app(screen, ui, AppKind::Settings, clock);
        return;
    }
    if point.1 >= 396 && point.1 <= 520 {
        // Tapping a notification just dismisses the shade so the launcher
        // is visible.
        toggle_shade(screen, ui, clock, false);
        return;
    }
    let _ = (height, clock);
}

fn toggle_shade(screen: &Screen, ui: &mut Ui, clock: [u64; 7], open: bool) {
    if ui.scene == Scene::App(AppKind::Browser) {
        if open {
            apps::browser_pause(&mut ui.apps);
        } else {
            apps::browser_resume(&mut ui.apps);
        }
    }
    ui.shade = open;
    ui.paint_bg = true;
    render(screen, ui, clock);
}

fn unlock(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    let height = screen.height();
    if ui.settings.animations {
        for step in 0..=6 {
            let offset = height * (100 - ease(step, 6)) / 100;
            screen.reset_dirty();
            draw_wallpaper(screen, 0, true);
            draw_lock(screen, clock, 0);
            draw_home(screen, clock, ui, None, offset, true);
            draw_status_bar(screen, clock, ui);
            present_dirty(screen);
            k::sleep_ms(12);
        }
    }
    ui.scene = Scene::Home;
    ui.paint_bg = true;
    render(screen, ui, clock);
}

fn open_app(screen: &Screen, ui: &mut Ui, app: AppKind, clock: [u64; 7]) {
    let width = screen.width();
    if ui.scene == Scene::App(AppKind::Browser) && app != AppKind::Browser {
        apps::browser_pause(&mut ui.apps);
    }
    apps::open(&mut ui.apps, app);
    ui.scroll_accum = 0;
    ui.push_recent(app);
    ui.folder_view = None;
    ui.edit_mode = false;
    ui.drag = None;
    ui.osk = match app {
        AppKind::Terminal | AppKind::AiStudio => true,
        AppKind::Notes | AppKind::Browser | AppKind::Files | AppKind::Market => apps::text_focused(&ui.apps, app),
        _ => false,
    };
    ui.shift = false;
    if ui.settings.animations {
        for step in 0..=6 {
            let offset = width * (100 - ease(step, 6)) / 100;
            screen.reset_dirty();
            draw_wallpaper(screen, 0, true);
            draw_home(screen, clock, ui, None, 0, true);
            draw_app(screen, ui, app, clock, offset, true);
            draw_status_bar(screen, clock, ui);
            present_dirty(screen);
            k::sleep_ms(12);
        }
    }
    ui.scene = Scene::App(app);
    ui.paint_bg = true;
    render(screen, ui, clock);
}

fn close_app(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    let width = screen.width();
    if let Scene::App(app) = ui.scene {
        if app == AppKind::Browser {
            apps::browser_pause(&mut ui.apps);
        }
        if ui.settings.animations {
            for step in 0..=6 {
                let offset = width * ease(step, 6) / 100;
                screen.reset_dirty();
                draw_wallpaper(screen, 0, true);
                draw_home(screen, clock, ui, None, 0, true);
                draw_app(screen, ui, app, clock, offset, true);
                draw_status_bar(screen, clock, ui);
                present_dirty(screen);
            k::sleep_ms(12);
            }
        }
    }
    ui.scene = Scene::Home;
    ui.osk = false;
    ui.shift = false;
    ui.drag = None;
    if ui.home_page == 2 {
        apps::launcher_refresh(&mut ui.apps);
    }
    ui.paint_bg = true;
    render(screen, ui, clock);
}

fn open_recents(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    if let Scene::App(AppKind::Browser) = ui.scene {
        apps::browser_pause(&mut ui.apps);
    }
    screen.set_app_space(false);
    let height = screen.height();
    ui.osk = false;
    ui.shade = false;
    ui.edit_mode = false;
    ui.drag = None;
    ui.folder_view = None;
    ui.recents_scroll = 0;
    if ui.settings.animations {
        for step in 0..=6 {
            let offset = height * (100 - ease(step, 6)) / 100;
            screen.reset_dirty();
            draw_wallpaper(screen, 0, ui.settings.dark);
            draw_recents(screen, ui, offset);
            draw_status_bar(screen, clock, ui);
            present_dirty(screen);
            k::sleep_ms(12);
        }
    }
    ui.scene = Scene::Recents;
    ui.paint_bg = true;
    render(screen, ui, clock);
}

fn go_home(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    screen.set_app_space(false);
    let height = screen.height();
    if ui.settings.animations {
        for step in 0..=6 {
            let offset = height * ease(step, 6) / 100;
            screen.reset_dirty();
            draw_wallpaper(screen, 0, ui.settings.dark);
            draw_recents(screen, ui, offset);
            draw_status_bar(screen, clock, ui);
            present_dirty(screen);
            k::sleep_ms(12);
        }
    }
    ui.scene = Scene::Home;
    if ui.home_page == 2 {
        apps::launcher_refresh(&mut ui.apps);
    }
    ui.paint_bg = true;
    render(screen, ui, clock);
}

fn render(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    screen.reset_dirty();
    let paint = ui.paint_bg;
    draw_scene(screen, ui, clock, paint);
    if ui.shade {
        draw_shade(screen, ui, clock, 0);
    } else {
        draw_status_bar(screen, clock, ui);
    }
    if let Some(point) = ui.press.filter(|_| ui.drag.is_none()) {
        screen.ring(point.0, point.1, 18, 3, 0x00a0_a8b8);
        screen.circle(point.0, point.1, 3, TEXT());
    }
    ui.paint_bg = false;
    present_dirty(screen);
}

/// Copy only the drawn (dirty) region of the back buffer to the display.
fn present_dirty(screen: &Screen) {
    if let Some(d) = screen.dirty_rect() {
        k::fb_present(d[0] as u64, d[1] as u64, d[2] as u64, d[3] as u64);
    }
}

fn draw_scene(screen: &Screen, ui: &mut Ui, clock: [u64; 7], paint_bg: bool) {
    match ui.scene {
        Scene::Lock => {
            draw_wallpaper(screen, 0, true);
            draw_lock(screen, clock, 0);
        }
        Scene::Home => {
            let pressed = ui
                .press
                .and_then(|point| home_slot_hit(screen, ui, point))
                .map(|(_, slot)| slot);
            draw_home(screen, clock, ui, pressed, 0, paint_bg);
            if ui.searching && ui.osk {
                screen.set_app_space(true);
                let pressed = ui
                    .press
                    .map(|p| (p.0, p.1 * 800 / screen.logical_height()));
                keyboard::draw(screen, ui.shift, ui.symbols, pressed);
                screen.set_app_space(false);
            }
        }
        Scene::App(app) => {
            draw_app(screen, ui, app, clock, 0, paint_bg);
        }
        Scene::Recents => {
            draw_recents(screen, ui, 0);
        }
    }
}

static mut WP_CACHE: [u32; 1080 * 1920] = [0; 1080 * 1920];
static mut WP_STATE: u8 = 0; // 0 invalid, 1 dark, 2 light

fn draw_wallpaper(screen: &Screen, y_offset: i64, dark: bool) {
    let (pw, ph) = screen.physical_size();
    // Full-screen wallpaper is static: render the fill + dot grid once into a
    // cache, then blit it on repaints (animation frames) instead of redrawing
    // hundreds of dots every frame.
    if y_offset == 0 && pw == 1080 && ph == 1920 {
        unsafe {
            let want = if dark { 1 } else { 2 };
            if WP_STATE != want {
                let cs = Screen::new(WP_CACHE.as_mut_ptr() as u64, 1080, 1920, 1080 * 4);
                paint_wallpaper(&cs, 0, dark);
                WP_STATE = want;
            }
            let dst = screen.back_ptr();
            let stride = screen.stride_pixels();
            for y in 0..1920usize {
                core::ptr::copy_nonoverlapping(
                    WP_CACHE.as_ptr().add(y * 1080),
                    dst.add(y * stride as usize),
                    1080,
                );
            }
            screen.mark_full_dirty();
            return;
        }
    }
    paint_wallpaper(screen, y_offset, dark);
}

fn paint_wallpaper(screen: &Screen, y_offset: i64, dark: bool) {
    let color = if dark { 0x0004_0406 } else { 0x00dc_dce6 };
    screen.fill(0, y_offset, screen.width(), screen.height() - y_offset, color);
    if dark {
        // Nothing-style dot grid.
        let mut y = 40 + y_offset;
        while y < screen.height() {
            let mut x = 16;
            while x < screen.width() {
                screen.fill(x, y, 2, 2, 0x0015_151b);
                x += 24;
            }
            y += 24;
        }
    }
}

// --- Fast icon drag: snapshot the static home once, then per move only blit
// the snapshot + redraw the moving icon (avoids re-rendering all supersampled
// text every touch event, which made dragging unusably laggy). ---
static DRAG_BG_CAPTURE: AtomicU64 = AtomicU64::new(0);
static mut DRAG_BG: [u32; 1080 * 1920] = [0; 1080 * 1920];
static mut DRAG_BG_VALID: bool = false;

fn loc_center(loc: Loc, width: i64, height: i64) -> (i64, i64) {
    match loc {
        Loc::Grid(i) | Loc::Grid2(i) => grid_center(width, height, i),
        Loc::Dock(i) => (76 + i as i64 * 109, height - 120 + 46),
        Loc::Folder(_, _) => (width / 2, height / 2),
    }
}

fn draw_drag_overlay(screen: &Screen, ui: &Ui) {
    let (width, height) = (screen.width(), screen.height());
    let (_, _, icon) = grid_metrics(height);
    if let Some(hover) = ui.drag.and_then(|d| d.hover) {
        let (cx, cy) = loc_center(hover, width, height);
        let r = if matches!(hover, Loc::Dock(_)) { 36 } else { icon / 2 + 8 };
        screen.ring(cx, cy, r, 3, RED());
    }
    if let Some(drag) = ui.drag {
        let size = icon + 10;
        screen.round_rect(
            ui.touch.0 - size / 2 - 4, ui.touch.1 - size / 2 - 4,
            size + 8, size + 8, size / 4, 0x0000_0000,
        );
        draw_slot_icon(screen, ui, drag.slot, ui.touch.0, ui.touch.1, size, true);
    }
}

fn drag_render(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    let (pw, ph) = screen.physical_size();
    if pw != 1080 || ph != 1920 {
        render(screen, ui, clock);
        return;
    }
    unsafe {
        let stride = screen.stride_pixels() as usize;
        if !DRAG_BG_VALID {
            // Capture the static home (origin slot empty, no overlay) once.
            DRAG_BG_CAPTURE.store(1, Ordering::Relaxed);
            ui.paint_bg = true;
            render(screen, ui, clock);
            DRAG_BG_CAPTURE.store(0, Ordering::Relaxed);
            let src = screen.back_ptr();
            for y in 0..1920usize {
                core::ptr::copy_nonoverlapping(src.add(y * stride), DRAG_BG.as_mut_ptr().add(y * 1080), 1080);
            }
            DRAG_BG_VALID = true;
        } else {
            let dst = screen.back_ptr();
            for y in 0..1920usize {
                core::ptr::copy_nonoverlapping(DRAG_BG.as_ptr().add(y * 1080), dst.add(y * stride), 1080);
            }
            screen.reset_dirty();
        }
        screen.set_app_space(false);
        draw_drag_overlay(screen, ui);
        screen.mark_full_dirty();
        present_dirty(screen);
    }
}

fn draw_status_bar(screen: &Screen, clock: [u64; 7], ui: &Ui) {
    screen.set_app_space(false);
    let width = screen.width();
    screen.fill(0, 0, width, 28, BLACK());

    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    screen.text(16, 7, str_of(&time), 2, TEXT());

    // Notification dot.
    screen.circle(120, 14, 3, RED());

    // Signal bars.
    for index in 0..4 {
        let bar = 4 + index * 4;
        screen.fill(
            width - 104 + index * 7,
            22 - bar,
            4,
            bar,
            if index < 3 { TEXT() } else { MUTED() },
        );
    }
    screen.text(width - 76, 7, "5G", 1, MUTED());

    // Battery.
    let battery_x = width - 44;
    screen.round_rect(battery_x, 8, 30, 15, 4, TEXT());
    screen.round_rect(battery_x + 2, 10, 26, 11, 3, BLACK());
    let charge = 20 * (if ui.settings.torch { 5 } else { 14 }) / 20;
    screen.round_rect(battery_x + 4, 12, charge, 7, 2, GREEN());
    screen.fill(battery_x + 31, 12, 2, 7, TEXT());
}

fn draw_lock(screen: &Screen, clock: [u64; 7], y_offset: i64) {
    screen.set_app_space(false);
    let width = screen.width();
    let center = width / 2;

    let mut date = [0u8; 16];
    format_date(clock, &mut date);
    screen.text(28, screen.sy(58) + y_offset, str_of(&date), 2, MUTED());
    screen.text(28, screen.sy(82) + y_offset, "KIMIOS PREVIEW", 1, 0x0060_606c);

    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    let cells = 9;
    let clock_width = Screen::dots_text_width(str_of(&time), cells);
    screen.text_dots(
        center - clock_width / 2,
        screen.sy(120) + y_offset,
        str_of(&time),
        cells,
        3,
        TEXT(),
    );

    notification_card(
        screen,
        28,
        screen.sy(260) + y_offset,
        0x00b8_c4ff,
        "System",
        "KimiOS 0.1 is up to date",
        "2m",
    );

    // Fingerprint target: three broken ridges opening at the bottom.
    for (radius, inner, outer) in [(33i64, 6usize, 54usize), (25, 9, 51), (17, 12, 48)] {
        draw_arc(screen, center, screen.sy(540) + y_offset, radius, inner, outer, 3, RED());
    }
    screen.text_centered(center, screen.sy(590) + y_offset, "Touch to unlock", 2, TEXT());
    screen.text_centered(center, screen.sy(620) + y_offset, "tap / swipe up / enter", 1, 0x0060_606c);

    draw_home_indicator(screen);
}

fn notification_card(
    screen: &Screen,
    x: i64,
    y: i64,
    accent: u32,
    title: &str,
    body: &str,
    age: &str,
) {
    let width = screen.width() - 56;
    let height = screen.sy(92);
    screen.round_rect(x, y, width, height, 26, SURFACE());
    screen.circle(x + 38, y + height / 2, 16, accent);
    screen.circle(x + 38, y + height / 2, 6, BLACK());
    screen.text(x + 68, y + height * 22 / 100, title, 2, TEXT());
    screen.text(x + 68, y + height * 52 / 100, body, 1, MUTED());
    screen.text(x + width - 34, y + height * 24 / 100, age, 1, 0x0060_606c);
}

fn grid_metrics(height: i64) -> (i64, i64, i64) {
    let top = if height < 720 { height * 140 / 800 } else { height * 330 / 800 };
    let spacing = (height - 160 - top) / 3;
    let icon = spacing.saturating_sub(34).min(60).max(30);
    (top, spacing, icon)
}

fn draw_home(
    screen: &Screen,
    clock: [u64; 7],
    ui: &Ui,
    pressed: Option<Slot>,
    y_offset: i64,
    paint_bg: bool,
) {
    screen.set_app_space(false);
    let (width, height) = (screen.width(), screen.height());
    let compact = height < 720;

    if paint_bg {
        draw_wallpaper(screen, y_offset, ui.settings.dark);
    }

    // At-a-glance / clock widget.
    screen.round_rect(24, screen.sy(44) + y_offset, width - 48, screen.sy(82), 26, SURFACE());
    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    screen.text(48, screen.sy(56) + y_offset, str_of(&time), 3, TEXT());
    let mut date = [0u8; 16];
    format_date(clock, &mut date);
    screen.text(48, screen.sy(94) + y_offset, str_of(&date), 1, MUTED());
    let mut temp_buf = [0u8; 20];
    let (temp_text, glance_cond) = match apps::home_weather(&ui.apps.weather) {
        Some((temp, cond)) => (format_i64(temp, &mut temp_buf), cond),
        None => ("--", "NO DATA"),
    };
    let cond_len = glance_cond.len().min(12);
    screen.text(width - 48 - Screen::text_width(temp_text, 3), screen.sy(60) + y_offset, temp_text, 3, RED());
    screen.text(width - 76, screen.sy(96) + y_offset, &glance_cond[..cond_len], 1, MUTED());

    if !compact && ui.edit_mode && !ui.searching {
        screen.text(24, screen.sy(162) + y_offset, "Hold & drag to rearrange", 1, MUTED());
        screen.round_rect(width - 110, screen.sy(142) + y_offset, 86, screen.sy(54), 27, RED());
        screen.text_centered(width - 67, screen.sy(160) + y_offset, "DONE", 2, TEXT());
    } else if !compact {
        // Search pill.
        let pill_bg = if ui.searching { SURFACE_MAX() } else { SURFACE_HI() };
        screen.round_rect(24, screen.sy(142) + y_offset, width - 48, screen.sy(54), 27, pill_bg);
        screen.ring(54, screen.sy(169) + y_offset, 8, 2, MUTED());
        screen.line(60, screen.sy(175) + y_offset, 66, screen.sy(181) + y_offset, 2, MUTED());
        if ui.searching {
            if ui.search_len == 0 {
                screen.text(80, screen.sy(162) + y_offset, "Search the web", 2, MUTED());
            } else {
                let start = ui.search_len.saturating_sub(24);
                let shown = ui.search_len - start;
                screen.text(
                    80,
                    screen.sy(162) + y_offset,
                    str_of(&ui.search_query[start..start + shown]),
                    2,
                    TEXT(),
                );
            }
            screen.round_rect(width - 100, screen.sy(148) + y_offset, 72, screen.sy(42), 21, RED());
            screen.text_centered(width - 64, screen.sy(160) + y_offset, "GO", 2, TEXT());
        } else {
            screen.text(80, screen.sy(162) + y_offset, "Search", 2, MUTED());
        }

        // Music widget.
        let music_y = screen.sy(212) + y_offset;
        let (title, subtitle, playing, progress) = apps::home_music(&ui.apps.music);
        screen.round_rect(24, music_y, width - 48, screen.sy(100), 28, SURFACE());
        screen.round_rect(42, music_y + screen.sy(18), 64, 64, 20, SURFACE_HI());
        screen.circle(74, music_y + screen.sy(50), 18, RED_DIM());
        screen.circle(74, music_y + screen.sy(50), 7, RED());
        screen.text(122, music_y + screen.sy(20), title, 2, TEXT());
        screen.text(122, music_y + screen.sy(48), subtitle, 1, MUTED());
        draw_play(screen, width - 76, music_y + screen.sy(50), playing);
        screen.round_rect(122, music_y + screen.sy(76), width - 190, 4, 2, OUTLINE());
        screen.round_rect(122, music_y + screen.sy(76), (width - 190) * progress / 1000, 4, 2, RED());
    }

    // App grid (persisted layout, edit-mode jiggle + drag).
    let (_, _, icon) = grid_metrics(height);
    let jiggle = if ui.edit_mode { ((k::uptime_ms() / 140) % 2) as i64 * 2 - 1 } else { 0 };
    let drag_from = ui.drag.map(|d| d.from);
    let capturing = DRAG_BG_CAPTURE.load(Ordering::Relaxed) == 1;
    let drag_hover = if capturing { None } else { ui.drag.and_then(|d| d.hover) };
    if ui.home_page == 2 {
        let count = apps::launcher_count(&ui.apps);
        let pressed_tile = ui.press.and_then(|p| plugin_home_hit(screen, ui, p));
        for index in 0..12usize {
            if index > 0 && index - 1 >= count {
                continue;
            }
            let (cx, cy) = grid_center(width, height, index);
            let is_pressed = pressed_tile == Some(index);
            draw_app_icon(screen, AppKind::Plugins, cx, cy + y_offset, icon, is_pressed);
            let label = if index == 0 {
                "Manage"
            } else {
                let name = apps::launcher_name(&ui.apps, index - 1);
                &name[..name.len().min(10)]
            };
            screen.text_centered(cx, cy + icon / 2 + 12 + y_offset, label, 1, MUTED());
        }
        if count == 0 {
            let (_, cy) = grid_center(width, height, 5);
            screen.text_centered(width / 2, cy + y_offset, "No plugin apps yet", 2, MUTED());
        }
    } else {
        for (index, slot) in ui.layout.page(ui.home_page).iter().enumerate() {
            let loc = if ui.home_page == 1 { Loc::Grid2(index) } else { Loc::Grid(index) };
            if drag_from == Some(loc) {
                continue;
            }
            let (cx, cy) = grid_center(width, height, index);
            let cx = cx + jiggle;
            draw_slot_icon(screen, ui, *slot, cx, cy + y_offset, icon, pressed == Some(*slot));
            if drag_hover == Some(loc) {
                screen.ring(cx, cy + y_offset, icon / 2 + 8, 3, RED());
            }
            let label = slot_label(ui, *slot);
            screen.text_centered(cx, cy + icon / 2 + 12 + y_offset, label, 1, MUTED());
        }
    }

    // Page indicator (three pages).
    let dots_y = height - 152 + y_offset;
    for page in 0..3usize {
        let dx = width / 2 - 24 + page as i64 * 24;
        if page == ui.home_page {
            screen.round_rect(dx - 10, dots_y - 3, 20, 6, 3, RED());
        } else {
            screen.circle(dx, dots_y, 3, OUTLINE());
        }
    }

    // Dock.
    let dock_y = height - 120 + y_offset;
    screen.round_rect(20, dock_y, width - 40, 92, 34, SURFACE());
    for (index, slot) in ui.layout.dock.iter().enumerate() {
        if drag_from == Some(Loc::Dock(index)) {
            continue;
        }
        let cx = 76 + index as i64 * 109 + jiggle;
        draw_slot_icon(screen, ui, *slot, cx, dock_y + 46, 56, pressed == Some(*slot));
        if drag_hover == Some(Loc::Dock(index)) {
            screen.ring(cx, dock_y + 46, 36, 3, RED());
        }
    }

    if let Some(f) = ui.folder_view {
        draw_folder_view(screen, ui, f, y_offset);
    }

    if !capturing {
        if let Some(drag) = ui.drag {
            let size = icon + 10;
            screen.round_rect(
                ui.touch.0 - size / 2 - 4,
                ui.touch.1 - size / 2 - 4,
                size + 8,
                size + 8,
                size / 4,
                0x0000_0000,
            );
            draw_slot_icon(screen, ui, drag.slot, ui.touch.0, ui.touch.1, size, true);
        }
    }

    draw_home_indicator(screen);
}

fn slot_label(ui: &Ui, slot: Slot) -> &str {
    match slot {
        Slot::App(app) => app.label(),
        Slot::Folder(f) => ui.layout.folders[f as usize].name_str(),
        Slot::Empty => "",
    }
}

fn draw_slot_icon(screen: &Screen, ui: &Ui, slot: Slot, cx: i64, cy: i64, size: i64, pressed: bool) {
    match slot {
        Slot::App(app) => draw_app_icon(screen, app, cx, cy, size, pressed),
        Slot::Folder(f) => {
            let tile = if pressed { SURFACE_MAX() } else { SURFACE_HI() };
            screen.round_rect(cx - size / 2, cy - size / 2, size, size, size / 4, tile);
            let folder = &ui.layout.folders[f as usize];
            for m in 0..folder.count.min(4) {
                let col = (m % 2) as i64;
                let row = (m / 2) as i64;
                let gx = cx - size / 5 + col * size * 2 / 5;
                let gy = cy - size / 5 + row * size * 2 / 5;
                draw_glyph(screen, folder.members[m], gx, gy, size * 2 / 5, TEXT());
            }
        }
        Slot::Empty => {
            screen.ring(cx, cy, size / 3, 2, OUTLINE());
        }
    }
}

fn folder_panel_rect(screen: &Screen) -> (i64, i64, i64, i64) {
    (40, screen.sy(220), screen.width() - 80, screen.sy(400))
}

fn folder_member_pos(screen: &Screen, m: usize) -> (i64, i64) {
    let col = (m % 3) as i64;
    let row = (m / 3) as i64;
    (110 + col * 130, screen.sy(320) + row * screen.sy(140))
}

fn draw_folder_view(screen: &Screen, ui: &Ui, f: usize, y_offset: i64) {
    if f >= ui.layout.folder_count {
        return;
    }
    let (x, y, w, h) = folder_panel_rect(screen);
    screen.round_rect(x, y + y_offset, w, h, 30, SURFACE_HI());
    let folder = &ui.layout.folders[f];
    screen.text_centered(x + w / 2, y + y_offset + 18, folder.name_str(), 2, TEXT());
    for m in 0..folder.count {
        if ui.drag.map(|d| d.from) == Some(Loc::Folder(f, m)) {
            continue;
        }
        let (cx, cy) = folder_member_pos(screen, m);
        draw_app_icon(screen, folder.members[m], cx, cy + y_offset, 60, false);
        screen.text_centered(cx, cy + 42 + y_offset, folder.members[m].label(), 1, MUTED());
    }
    button_home(screen, x + 20, y + y_offset + h - 60, 130, 40, "Rename", SURFACE_MAX(), TEXT());
    screen.text(x + 170, y + y_offset + h - 48, "Tap outside to close", 1, MUTED());
    if ui.edit_mode {
        screen.text(x + 170, y + y_offset + h - 26, "Hold a member to drag it out", 1, MUTED());
    }
}

fn button_home(screen: &Screen, x: i64, y: i64, w: i64, h: i64, label: &str, bg: u32, fg: u32) {
    screen.round_rect(x, y, w, h, h / 2, bg);
    screen.text_centered(x + w / 2, y + h / 2 - 8, label, 2, fg);
}

fn draw_recents(screen: &Screen, ui: &Ui, y_offset: i64) {
    screen.set_app_space(false);
    let width = screen.width();
    let height = screen.height();
    draw_wallpaper(screen, 0, ui.settings.dark);
    screen.text(24, screen.sy(60) + y_offset, "RECENTS", 2, TEXT());
    screen.round_rect(width - 130, screen.sy(52) + y_offset, 106, 36, 18, SURFACE_HI());
    screen.text_centered(width - 77, screen.sy(62) + y_offset, "Clear all", 1, TEXT());
    if ui.recent_count == 0 {
        screen.text_centered(width / 2, screen.sy(380) + y_offset, "No recent apps", 2, MUTED());
        screen.text_centered(width / 2, screen.sy(420) + y_offset, "Open apps from the home screen", 1, MUTED());
    }
    for row in 0..4usize {
        let index = ui.recents_scroll + row;
        if index >= ui.recent_count {
            break;
        }
        let app = ui.recents[index];
        let y = screen.sy(120) + y_offset + row as i64 * (screen.sy(150) + 12);
        let card_h = screen.sy(150);
        screen.round_rect(40, y, width - 80, card_h, 24, SURFACE());
        screen.gradient(56, y + 14, width - 112, card_h - 28, SURFACE_HI(), SURFACE());
        draw_app_icon(screen, app, 92, y + card_h / 2, 56, false);
        screen.text(140, y + 24, app.label(), 2, TEXT());
        screen.text(140, y + 56, "Tap to open", 1, MUTED());
        screen.text(140, y + 78, "Swipe away to dismiss", 1, OUTLINE());
        draw_glyph(screen, app, width - 90, y + card_h / 2, 44, OUTLINE());
    }
    if ui.recent_count > 4 {
        button_home(screen, 40, height - 170 + y_offset, 90, 36, "up", SURFACE_HI(), MUTED());
        button_home(screen, width - 130, height - 170 + y_offset, 90, 36, "dn", SURFACE_HI(), MUTED());
    }
    screen.text_centered(width / 2, height - 60 + y_offset, "Swipe up for home", 1, MUTED());
    draw_home_indicator(screen);
}

fn draw_app_icon(screen: &Screen, app: AppKind, cx: i64, cy: i64, size: i64, pressed: bool) {
    let tile = if pressed { SURFACE_MAX() } else { SURFACE_HI() };
    screen.round_rect(cx - size / 2, cy - size / 2, size, size, size / 4, tile);
    screen.round_rect(
        cx - size / 2 + 1,
        cy - size / 2 + 1,
        size - 2,
        size - 2,
        size / 4 - 1,
        tile,
    );
    let glyph = if app == AppKind::Music { RED() } else { TEXT() };
    draw_glyph(screen, app, cx, cy, size, glyph);
}

fn draw_glyph(screen: &Screen, app: AppKind, cx: i64, cy: i64, size: i64, color: u32) {
    let s = size;
    let thin = (s / 16).max(2);
    match app {
        AppKind::Music => {
            screen.circle(cx - s / 10, cy + s / 6, s / 9, color);
            screen.line(cx - s / 20, cy + s / 8, cx - s / 20, cy - s / 5, s / 12, color);
            screen.line(cx - s / 20, cy - s / 5, cx + s / 5, cy - s / 4, s / 10, color);
        }
        AppKind::Clock => {
            screen.ring(cx, cy, s / 3, thin + 1, color);
            screen.line(cx, cy, cx, cy - s / 5, thin, color);
            screen.line(cx, cy, cx + s / 6, cy, thin, color);
            screen.circle(cx, cy, s / 24, color);
        }
        AppKind::Settings => {
            for index in 0..8 {
                let dx = CLOCK_X[index * 7 % 60] * s / 2500;
                let dy = CLOCK_Y[index * 7 % 60] * s / 2500;
                let tooth = s / 9;
                screen.round_rect(
                    cx + dx - tooth / 2,
                    cy + dy - tooth / 2,
                    tooth,
                    tooth,
                    tooth / 3,
                    color,
                );
            }
            screen.ring(cx, cy, s / 3, thin + 1, color);
            screen.circle(cx, cy, s / 10, color);
        }
        AppKind::Calculator => {
            screen.round_rect(cx - s / 4, cy - s / 3, s / 2, s * 2 / 3, s / 12, color);
            screen.round_rect(
                cx - s / 4 + 2,
                cy - s / 3 + 2,
                s / 2 - 4,
                s * 2 / 3 - 4,
                s / 16,
                SURFACE_HI(),
            );
            for row in 0..3 {
                for column in 0..3 {
                    screen.fill(
                        cx - s / 9 + column * s / 7,
                        cy - s / 8 + row * s / 7,
                        s / 14,
                        s / 14,
                        color,
                    );
                }
            }
        }
        AppKind::Notes => {
            screen.round_rect(cx - s / 3, cy - s / 3, s * 2 / 3, s * 2 / 3, s / 12, color);
            screen.round_rect(
                cx - s / 3 + 2,
                cy - s / 3 + 2,
                s * 2 / 3 - 4,
                s * 2 / 3 - 4,
                s / 16,
                SURFACE_HI(),
            );
            for row in 0..3 {
                screen.fill(cx - s / 5, cy - s / 6 + row * s / 6, s * 2 / 5, s / 22, color);
            }
        }
        AppKind::Maps => {
            screen.ring(cx, cy - s / 8, s / 4, thin, color);
            screen.line(cx - s / 8, cy + s / 24, cx, cy + s / 3, thin, color);
            screen.line(cx + s / 8, cy + s / 24, cx, cy + s / 3, thin, color);
            screen.circle(cx, cy - s / 8, s / 14, color);
        }
        AppKind::Photos => {
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                screen.circle(
                    cx + dx * s / 8,
                    cy + dy * s / 8,
                    s / 7,
                    color,
                );
            }
            screen.circle(cx, cy, s / 10, SURFACE_HI());
        }
        AppKind::Files => {
            screen.round_rect(cx - s / 3, cy - s / 5, s * 2 / 3, s / 2, s / 14, color);
            screen.round_rect(
                cx - s / 3 + 2,
                cy - s / 5 + 2,
                s * 2 / 3 - 4,
                s / 2 - 4,
                s / 18,
                SURFACE_HI(),
            );
            screen.fill(cx - s / 3, cy - s / 4, s / 3, s / 12, color);
        }
        AppKind::Weather => {
            let sx = cx - s / 12;
            let sy = cy - s / 8;
            screen.circle(sx, sy, s / 7, color);
            for index in 0..8 {
                let dx = CLOCK_X[index * 7 % 60] * s / 4200;
                let dy = CLOCK_Y[index * 7 % 60] * s / 4200;
                screen.line(
                    sx + dx,
                    sy + dy,
                    sx + dx * 3 / 2,
                    sy + dy * 3 / 2,
                    2,
                    color,
                );
            }
            screen.round_rect(cx - s / 6, cy + s / 12, s / 2, s / 5, s / 18, color);
        }
        AppKind::Terminal => {
            screen.round_rect(cx - s / 3, cy - s / 3, s * 2 / 3, s * 2 / 3, s / 16, color);
            screen.round_rect(
                cx - s / 3 + 2,
                cy - s / 3 + 2,
                s * 2 / 3 - 4,
                s * 2 / 3 - 4,
                s / 18,
                SURFACE_HI(),
            );
            screen.line(cx - s / 6, cy - s / 6, cx, cy, 3, color);
            screen.line(cx, cy, cx - s / 6, cy + s / 6, 3, color);
            screen.line(cx + s / 12, cy + s / 6, cx + s / 5, cy + s / 6, 3, color);
        }
        AppKind::Browser => {
            screen.ring(cx, cy, s / 3, thin + 1, color);
            screen.line(cx - s / 3, cy, cx + s / 3, cy, thin, color);
            screen.line(cx, cy - s / 3, cx, cy + s / 3, thin, color);
            screen.ring(cx, cy, s / 6, thin, color);
        }
        AppKind::Apk => {
            screen.round_rect(cx - s / 4, cy - s / 3, s / 2, s * 2 / 3, s / 12, color);
            screen.round_rect(cx - s / 4 + 2, cy - s / 3 + 2, s / 2 - 4, s * 2 / 3 - 4, s / 16, SURFACE_HI());
            screen.line(cx - s / 8, cy - s / 6, cx - s / 8, cy + s / 6, 2, color);
            screen.line(cx + s / 8, cy - s / 6, cx + s / 8, cy + s / 6, 2, color);
            screen.line(cx - s / 8, cy, cx + s / 8, cy, 2, color);
        }
        AppKind::Dvm => {
            screen.round_rect(cx - s / 3, cy - s / 3, s * 2 / 3, s * 2 / 3, s / 12, color);
            screen.round_rect(cx - s / 3 + 2, cy - s / 3 + 2, s * 2 / 3 - 4, s * 2 / 3 - 4, s / 16, SURFACE_HI());
            screen.line(cx - s / 10, cy - s / 6, cx - s / 10, cy + s / 6, 3, color);
            screen.line(cx - s / 10, cy - s / 6, cx + s / 6, cy, 3, color);
            screen.line(cx - s / 10, cy + s / 6, cx + s / 6, cy, 3, color);
        }
        AppKind::Market => {
            for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                screen.round_rect(cx + dx * s / 5 - s / 8, cy + dy * s / 5 - s / 8, s / 4, s / 4, s / 16, color);
            }
        }
        AppKind::Monitor => {
            let base = cy + s / 4;
            for (i, h) in [s / 4, s / 2, s / 3, s * 3 / 5].iter().enumerate() {
                let bx = cx - s / 3 + i as i64 * s / 5;
                screen.round_rect(bx, base - h, s / 7, *h, s / 20, color);
            }
            screen.fill(cx - s / 3, base + 2, s * 2 / 3, 2, color);
        }
        AppKind::Plugins => {
            screen.round_rect(cx - s / 3, cy - s / 6, s * 2 / 3, s / 2, s / 12, color);
            screen.circle(cx, cy - s / 6, s / 8, color);
            screen.circle(cx + s / 3, cy + s / 12, s / 8, color);
            screen.circle(cx - s / 3, cy + s / 12, s / 8, SURFACE_HI());
        }
        AppKind::AiStudio => {
            screen.round_rect(cx - s / 3, cy - s / 3, s * 2 / 3, s / 2, s / 8, color);
            screen.fill(cx - s / 8, cy + s / 6, s / 6, s / 8, color);
            screen.line(cx, cy - s / 5, cx, cy + s / 12, 3, SURFACE_HI());
            screen.line(cx - s / 6, cy - s / 12, cx + s / 6, cy - s / 12, 3, SURFACE_HI());
            screen.circle(cx + s / 4, cy - s / 4, s / 14, RED());
        }
    }
}

fn draw_play(screen: &Screen, cx: i64, cy: i64, playing: bool) {
    screen.circle(cx, cy, 21, SURFACE());
    screen.ring(cx, cy, 20, 2, OUTLINE());
    if playing {
        screen.fill(cx - 6, cy - 8, 4, 16, TEXT());
        screen.fill(cx + 2, cy - 8, 4, 16, TEXT());
    } else {
        screen.line(cx - 4, cy - 8, cx - 4, cy + 8, 2, TEXT());
        screen.line(cx - 4, cy - 8, cx + 8, cy, 2, TEXT());
        screen.line(cx - 4, cy + 8, cx + 8, cy, 2, TEXT());
        screen.line(cx - 2, cy, cx + 7, cy, 2, TEXT());
    }
}

fn draw_arc(
    screen: &Screen,
    cx: i64,
    cy: i64,
    radius: i64,
    from: usize,
    to: usize,
    thickness: i64,
    color: u32,
) {
    let mut previous = (
        cx + CLOCK_X[from] * radius / 1024,
        cy + CLOCK_Y[from] * radius / 1024,
    );
    for index in from + 1..=to {
        let point = (
            cx + CLOCK_X[index.min(59)] * radius / 1024,
            cy + CLOCK_Y[index.min(59)] * radius / 1024,
        );
        screen.line(previous.0, previous.1, point.0, point.1, thickness, color);
        previous = point;
    }
}

fn draw_home_indicator(screen: &Screen) {
    let width = screen.width();
    screen.round_rect(width / 2 - 62, screen.height() - 20, 124, 5, 2, TEXT());
}

fn draw_app(
    screen: &Screen,
    ui: &mut Ui,
    app: AppKind,
    clock: [u64; 7],
    x_offset: i64,
    paint_bg: bool,
) {
    screen.set_app_space(true);
    let x = x_offset;
    let width = screen.width();
    let height = screen.height();
    if paint_bg {
        screen.fill(x, 0, width - x, height, BLACK());
    }

    // Large Material 3 top app bar.
    draw_back(screen, x + 40, 64);
    screen.text(x + 78, 48, app.label(), 2, TEXT());
    screen.fill(x, 92, width, 1, 0x0020_2026);

    let osk = x_offset == 0 && ui.osk && apps::text_focused(&ui.apps, app);
    if app == AppKind::Monitor {
        apps::monitor_sync(&mut ui.apps, &ui.recents[..ui.recent_count], ui.fps_x10);
    }
    match app {
        AppKind::Calculator => draw_calculator(screen, ui, x),
        AppKind::Clock => draw_clock_app(screen, clock, x),
        AppKind::Settings => draw_settings(screen, &ui.settings, x),
        _ => apps::draw(screen, &mut ui.apps, app, clock, osk),
    }
    if osk {
        let pressed = ui
            .press
            .map(|p| (p.0, p.1 * 800 / screen.logical_height()));
        keyboard::draw(screen, ui.shift, ui.symbols, pressed);
    }
    draw_home_indicator(screen);
}

fn draw_back(screen: &Screen, cx: i64, cy: i64) {
    screen.line(cx + 9, cy - 12, cx - 9, cy, 3, TEXT());
    screen.line(cx - 9, cy, cx + 9, cy + 12, 3, TEXT());
}

fn draw_calculator(screen: &Screen, ui: &Ui, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    screen.fill(x, 93, width, screen.height() - 93, BLACK());

    screen.round_rect(x + 20, 112, width - 40, 148, 30, SURFACE());
    let mut digits = [0u8; 20];
    let text = format_i64(ui.calc.current, &mut digits);
    let scale = if text.len() > 9 { 3 } else { 4 };
    let text_width = Screen::text_width(text, scale);
    screen.text(x + width - 44 - text_width, 176, text, scale, TEXT());
    if let Some(op) = ui.calc.pending {
        screen.text(x + width - 44 - 20, 136, str_of(&[op]), 2, RED());
    }

    for row in 0..5usize {
        for column in 0..4usize {
            let label = CALCULATOR_LABELS[row * 4 + column];
            if label.is_empty() {
                continue;
            }
            let (bx, by, bw, bh) = calculator_button(x, row, column);
            let (bw, color, text_color) = if label == "=" {
                (bw * 2 + 10, RED(), TEXT())
            } else if label == "AC" {
                (bw, SURFACE_MAX(), RED())
            } else {
                (bw, SURFACE_HI(), TEXT())
            };
            let pressed = ui.press.map_or(false, |point| {
                point.0 >= bx && point.0 < bx + bw && point.1 >= by && point.1 < by + bh
            });
            let color = if pressed { SURFACE_MAX() } else { color };
            screen.round_rect(bx, by, bw, bh, 26, color);
            screen.text_centered(bx + bw / 2, by + bh / 2 - 11, label, 3, text_color);
        }
    }
}

const CALCULATOR_LABELS: [&str; 20] = [
    "AC", "+/-", "<", "/", "7", "8", "9", "*", "4", "5", "6", "-", "1", "2", "3", "+", "0",
    ".", "=", "",
];

fn calculator_button(x_offset: i64, row: usize, column: usize) -> (i64, i64, i64, i64) {
    let margin = 16;
    let gap = 10;
    let button_width = (480 - margin * 2 - gap * 3) / 4;
    let button_height = 86;
    let bx = x_offset + margin + column as i64 * (button_width + gap);
    let by = 280 + row as i64 * (button_height + 10);
    (bx, by, button_width, button_height)
}

fn calculator_hit(point: (i64, i64)) -> Option<usize> {
    for row in 0..5usize {
        for column in 0..4usize {
            let label = CALCULATOR_LABELS[row * 4 + column];
            if label.is_empty() {
                continue;
            }
            let (bx, by, bw, bh) = calculator_button(0, row, column);
            let bw = if label == "=" { bw * 2 + 10 } else { bw };
            if point.0 >= bx && point.0 < bx + bw && point.1 >= by && point.1 < by + bh {
                return Some(row * 4 + column);
            }
        }
    }
    None
}

fn draw_clock_app(screen: &Screen, clock: [u64; 7], x_offset: i64) {
    let x = x_offset;
    let cx = x + screen.width() / 2;
    let cy = 380;
    let radius = 150;

    screen.fill(x, 93, screen.width(), screen.height() - 93, BLACK());

    // Minute dots around the face, Nothing style.
    for index in 0..60 {
        let dx = CLOCK_X[index];
        let dy = CLOCK_Y[index];
        let big = index % 5 == 0;
        screen.circle(
            cx + dx * (radius - 10) / 1024,
            cy + dy * (radius - 10) / 1024,
            if big { 3 } else { 1 },
            if big { TEXT() } else { OUTLINE() },
        );
    }

    let hour = clock[4] % 12;
    let hour_index = (hour * 5 + clock[5] / 12) as usize;
    screen.line(
        cx,
        cy,
        cx + CLOCK_X[hour_index] * 72 / 1024,
        cy + CLOCK_Y[hour_index] * 72 / 1024,
        8,
        TEXT(),
    );
    let minute_index = clock[5] as usize;
    screen.line(
        cx,
        cy,
        cx + CLOCK_X[minute_index] * 112 / 1024,
        cy + CLOCK_Y[minute_index] * 112 / 1024,
        5,
        TEXT(),
    );
    let second_index = clock[6] as usize;
    screen.line(
        cx,
        cy,
        cx + CLOCK_X[second_index] * 124 / 1024,
        cy + CLOCK_Y[second_index] * 124 / 1024,
        2,
        RED(),
    );
    screen.circle(cx, cy, 7, RED());

    let mut time = [0u8; 8];
    let length = format_hms(clock[4], clock[5], clock[6], &mut time);
    let cell = 6;
    let text = str_of(&time[..length]);
    let width = Screen::dots_text_width(text, cell);
    screen.text_dots(cx - width / 2, 570, text, cell, 2, TEXT());

    let mut date = [0u8; 16];
    format_date(clock, &mut date);
    screen.text_centered(cx, 650, str_of(&date), 2, MUTED());
}

fn draw_settings(screen: &Screen, settings: &Settings, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    screen.fill(x, 93, width, screen.height() - 93, BLACK());

    screen.text(x + 24, 112, "CONNECTIVITY", 1, MUTED());
    let rows = [
        ("Wi-Fi", "KimiOS-5G", settings.wifi, RED()),
        ("Bluetooth", "Not connected", settings.bluetooth, 0x00b8_c4ff),
        ("Do Not Disturb", "Off", settings.dnd, 0x00d8_b060),
    ];
    for (index, (label, subtitle, value, accent)) in rows.iter().enumerate() {
        let y = 136 + index as i64 * 100;
        screen.round_rect(x + 20, y, width - 40, 88, 28, SURFACE());
        screen.circle(x + 58, y + 44, 18, *accent);
        screen.text(x + 88, y + 24, label, 2, TEXT());
        screen.text(x + 88, y + 52, subtitle, 1, MUTED());
        draw_switch(screen, x + width - 92, y + 28, *value);
    }

    screen.text(x + 24, 456, "APPEARANCE", 1, MUTED());
    screen.round_rect(x + 20, 480, width - 40, 168, 28, SURFACE());
    screen.circle(x + 58, 524, 18, 0x009c_74e8);
    screen.text(x + 88, 504, "Dark mode", 2, TEXT());
    screen.text(x + 88, 532, "Nothing palette", 1, MUTED());
    draw_switch(screen, x + width - 92, 508, settings.dark);
    screen.fill(x + 40, 560, width - 80, 1, OUTLINE());
    screen.circle(x + 58, 604, 18, 0x00d8_9040);
    screen.text(x + 88, 584, "UI animations", 2, TEXT());
    screen.text(x + 88, 612, "Experimental - disabled by default", 1, 0x00d8_9040);
    draw_switch(screen, x + width - 92, 588, settings.animations);

    screen.round_rect(x + 20, 660, width - 40, 96, 28, SURFACE());
    screen.text(x + 44, 680, "About KimiOS", 2, TEXT());
    screen.text(x + 44, 710, "ARM64 bare metal", 1, MUTED());
    screen.text(x + 44, 730, "1080x1920 - Material 3 / Nothing", 1, MUTED());
}

fn draw_switch(screen: &Screen, x: i64, y: i64, on: bool) {
    let track = if on { RED_DIM() } else { SURFACE_MAX() };
    screen.round_rect(x, y, 52, 32, 16, track);
    if on {
        screen.round_rect(x, y, 52, 32, 16, RED_DIM());
    }
    let knob = if on { x + 38 } else { x + 14 };
    screen.circle(knob, y + 16, 11, if on { RED() } else { MUTED() });
}

fn settings_hit(screen: &Screen, point: (i64, i64)) -> Option<usize> {
    for index in 0..3 {
        let y = 136 + index as i64 * 100;
        if point.0 >= 20
            && point.0 < screen.width() - 20
            && point.1 >= y
            && point.1 < y + 88
        {
            return Some(index);
        }
    }
    if point.0 >= 20 && point.0 < screen.width() - 20 && point.1 >= 480 && point.1 < 568 {
        return Some(3);
    }
    if point.0 >= 20 && point.0 < screen.width() - 20 && point.1 >= 584 && point.1 < 648 {
        return Some(4);
    }
    None
}

fn toggle_settings(settings: &mut Settings, row: usize) {
    match row {
        0 => settings.wifi = !settings.wifi,
        1 => settings.bluetooth = !settings.bluetooth,
        2 => settings.dnd = !settings.dnd,
        3 => settings.dark = !settings.dark,
        _ => settings.animations = !settings.animations,
    }
}

fn draw_placeholder(screen: &Screen, app: AppKind, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    let cx = x + width / 2;
    screen.fill(x, 93, width, screen.height() - 93, BLACK());
    screen.round_rect(cx - 52, 250, 104, 104, 30, SURFACE());
    draw_glyph(screen, app, cx, 302, 100, TEXT());
    screen.text_centered(cx, 400, app.label(), 3, TEXT());
    screen.round_rect(cx - 110, 460, 220, 52, 26, SURFACE());
    screen.text_centered(cx, 476, "Coming soon", 2, MUTED());
    screen.text_centered(cx, 540, "Material 3 / Nothing concept", 1, 0x0060_606c);
}

fn draw_shade(screen: &Screen, ui: &Ui, clock: [u64; 7], y_offset: i64) {
    screen.set_app_space(true);
    let width = screen.width();
    let height = screen.height();
    let sheet = 610;

    // Scrim only over the part the sheet does not cover.
    let scrim_top = (610 + y_offset).clamp(0, height);
    if scrim_top < height {
        screen.fill_blend(0, scrim_top, width, height - scrim_top, BLACK(), 160);
    }

    // Sheet with rounded bottom corners.
    screen.round_rect(0, y_offset - 40, width, sheet + 40, 40, 0x000a_0a0f);
    screen.round_rect_blend(0, y_offset + 40, width, sheet - 40, 0, 0x000a_0a0f, 255);

    screen.round_rect(width / 2 - 20, y_offset + 12, 40, 4, 2, 0x0050_505a);

    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    screen.text(28, y_offset + 30, str_of(&time), 2, TEXT());
    let title = "Quick settings";
    let title_x = (width - Screen::text_width(title, 1)) / 2;
    screen.text(title_x, y_offset + 34, title, 1, MUTED());

    for index in 0..4 {
        let column = index % 2;
        let row = index / 2;
        let x = 24 + column as i64 * 220;
        let y = y_offset + 72 + row as i64 * 108;
        let enabled = match index {
            0 => ui.settings.wifi,
            1 => ui.settings.bluetooth,
            2 => ui.settings.dnd,
            _ => ui.settings.torch,
        };
        let (title, subtitle) = match index {
            0 => ("Wi-Fi", if enabled { "KimiOS-5G" } else { "Off" }),
            1 => ("Bluetooth", if enabled { "On" } else { "Off" }),
            2 => ("Do Not Disturb", if enabled { "On" } else { "Off" }),
            _ => ("Torch", if enabled { "On" } else { "Off" }),
        };
        let card = if enabled { RED_DIM() } else { SURFACE_HI() };
        screen.round_rect(x, y, 208, 96, 30, card);
        shade_glyph(screen, index, x + 26, y + 30, enabled);
        screen.text(x + 22, y + 56, title, 1, TEXT());
        let subtitle_x = x + 208 - 14 - Screen::text_width(subtitle, 1);
        screen.text(subtitle_x, y + 56, subtitle, 1, MUTED());
    }

    // Brightness slider.
    screen.text(24, y_offset + 300, "BRIGHTNESS", 1, MUTED());
    screen.round_rect(24, y_offset + 322, width - 48, 22, 11, SURFACE_HI());
    let fill = (width - 48) * ui.settings.brightness / 100;
    screen.round_rect(24, y_offset + 322, fill, 22, 11, TEXT());
    screen.circle(24 + fill, y_offset + 333, 14, RED());

    screen.text(24, y_offset + 366, "NOTIFICATIONS", 1, MUTED());
    notification_card(
        screen,
        24,
        y_offset + 390,
        0x00b8_c4ff,
        "System",
        "KimiOS 0.1 is up to date",
        "2m",
    );

    // Settings shortcut in the header.
    screen.ring(width - 42, y_offset + 40, 11, 2, TEXT());
    screen.circle(width - 42, y_offset + 40, 3, TEXT());
}

fn shade_glyph(screen: &Screen, index: usize, x: i64, y: i64, enabled: bool) {
    let color = if enabled { TEXT() } else { MUTED() };
    match index {
        0 => {
            let base = y + 8;
            for radius in [20i64, 14, 8] {
                let mut previous: Option<(i64, i64)> = None;
                for step in -10..=10i64 {
                    let index = ((step * 6 / 6 + 60) % 60) as usize;
                    let point = (
                        x + CLOCK_X[index] * radius / 1024,
                        base + CLOCK_Y[index] * radius / 1024,
                    );
                    if let Some(previous) = previous {
                        screen.line(previous.0, previous.1, point.0, point.1, 2, color);
                    }
                    previous = Some(point);
                }
            }
            screen.circle(x, base, 2, color);
        }
        1 => {
            screen.line(x - 2, y - 16, x - 2, y + 2, 2, color);
            screen.line(x + 8, y - 16, x - 2, y - 16, 2, color);
            screen.line(x + 8, y - 16, x + 8, y - 8, 2, color);
            screen.line(x + 8, y - 8, x - 2, y - 8, 2, color);
            screen.line(x + 8, y - 8, x + 10, y - 4, 2, color);
            screen.line(x + 10, y - 4, x + 10, y + 2, 2, color);
            screen.line(x + 10, y + 2, x - 2, y + 2, 2, color);
        }
        2 => {
            screen.circle(x, y - 2, 12, color);
            screen.circle(x + 7, y - 6, 12, 0x000a0a0f);
        }
        _ => {
            screen.circle(x, y - 6, 10, color);
            screen.fill(x - 5, y - 2, 10, 16, color);
            screen.line(x - 8, y + 14, x + 8, y + 14, 2, color);
        }
    }
}

fn shade_toggle_hit(point: (i64, i64)) -> Option<usize> {
    for index in 0..4 {
        let column = index % 2;
        let row = index / 2;
        let x = 24 + column as i64 * 220;
        let y = 72 + row as i64 * 108;
        if point.0 >= x && point.0 < x + 208 && point.1 >= y && point.1 < y + 96 {
            return Some(index);
        }
    }
    None
}

fn back_hit(_screen: &Screen, point: (i64, i64)) -> bool {
    let bottom = 800 - 30;
    (point.0 < 76 && point.1 > 30 && point.1 < 96) || point.1 > bottom
}

fn music_hit(screen: &Screen, point: (i64, i64)) -> bool {
    if screen.height() < 720 {
        return false;
    }
    point.0 >= screen.width() - 108
        && point.0 <= screen.width() - 44
        && point.1 >= 224
        && point.1 <= 300
}

fn grid_center(width: i64, height: i64, index: usize) -> (i64, i64) {
    let (top, spacing, _) = grid_metrics(height);
    let column = index % 4;
    let row = index / 4;
    let cx = width / 2 + (column as i64 - 1) * 112 - 56;
    let cy = top + spacing / 2 + row as i64 * spacing;
    (cx, cy)
}

fn home_page_hit(screen: &Screen, point: (i64, i64)) -> Option<usize> {
    let width = screen.width();
    let dots_y = screen.height() - 152;
    if (point.1 - dots_y).abs() > 16 {
        return None;
    }
    for page in 0..3usize {
        let dx = width / 2 - 24 + page as i64 * 24;
        if (point.0 - dx).abs() <= 14 {
            return Some(page);
        }
    }
    None
}

fn plugin_home_hit(screen: &Screen, ui: &Ui, point: (i64, i64)) -> Option<usize> {
    if ui.home_page != 2 {
        return None;
    }
    let width = screen.width();
    let height = screen.logical_height();
    let (_, _, icon) = grid_metrics(height);
    let reach = icon / 2 + 10;
    let count = apps::launcher_count(&ui.apps);
    for index in 0..12usize {
        if index > 0 && index - 1 >= count {
            continue;
        }
        let (cx, cy) = grid_center(width, height, index);
        if (point.0 - cx).abs() <= reach && (point.1 - cy).abs() <= reach {
            return Some(index);
        }
    }
    None
}

fn home_slot_hit(screen: &Screen, ui: &Ui, point: (i64, i64)) -> Option<(Loc, Slot)> {
    let width = screen.width();
    let height = screen.logical_height();
    if let Some(f) = ui.folder_view {
        if let Some(m) = folder_member_hit(screen, ui, f, point) {
            return Some((Loc::Folder(f, m), Slot::App(ui.layout.folders[f].members[m])));
        }
    }
    if ui.home_page == 2 {
        return None;
    }
    let (_, _, icon) = grid_metrics(height);
    let reach = icon / 2 + 10;
    for (index, slot) in ui.layout.page(ui.home_page).iter().enumerate() {
        let (cx, cy) = grid_center(width, height, index);
        if (point.0 - cx).abs() <= reach && (point.1 - cy).abs() <= reach {
            let loc = if ui.home_page == 1 { Loc::Grid2(index) } else { Loc::Grid(index) };
            return Some((loc, *slot));
        }
    }
    let dock_y = height - 120 + 46;
    for (index, slot) in ui.layout.dock.iter().enumerate() {
        let cx = 76 + index as i64 * 109;
        if (point.0 - cx).abs() <= 34 && (point.1 - dock_y).abs() <= 34 {
            return Some((Loc::Dock(index), *slot));
        }
    }
    None
}

fn folder_panel_hit(screen: &Screen, point: (i64, i64)) -> bool {
    let (x, y, w, h) = folder_panel_rect(screen);
    point.0 >= x && point.0 <= x + w && point.1 >= y && point.1 <= y + h
}

fn folder_member_hit(screen: &Screen, ui: &Ui, f: usize, point: (i64, i64)) -> Option<usize> {
    if f >= ui.layout.folder_count {
        return None;
    }
    for m in 0..ui.layout.folders[f].count {
        let (cx, cy) = folder_member_pos(screen, m);
        if (point.0 - cx).abs() <= 48 && (point.1 - cy).abs() <= 48 {
            return Some(m);
        }
    }
    None
}

fn folder_rename_hit(screen: &Screen, point: (i64, i64)) -> bool {
    let (x, y, _, h) = folder_panel_rect(screen);
    let by = y + h - 60;
    point.0 >= x + 20 && point.0 <= x + 150 && point.1 >= by && point.1 <= by + 40
}

fn edit_done_hit(screen: &Screen, point: (i64, i64)) -> bool {
    let width = screen.width();
    point.0 >= width - 110
        && point.0 <= width - 24
        && point.1 >= screen.sy(142)
        && point.1 <= screen.sy(196)
}

fn recents_card_at(screen: &Screen, ui: &Ui, point: (i64, i64)) -> Option<usize> {
    let width = screen.width();
    for row in 0..4usize {
        let index = ui.recents_scroll + row;
        if index >= ui.recent_count {
            break;
        }
        let y = screen.sy(120) + row as i64 * (screen.sy(150) + 12);
        if point.0 >= 40 && point.0 <= width - 40 && point.1 >= y && point.1 <= y + screen.sy(150) {
            return Some(index);
        }
    }
    None
}

fn recents_clear_hit(screen: &Screen, point: (i64, i64)) -> bool {
    let width = screen.width();
    point.0 >= width - 130
        && point.0 <= width - 24
        && point.1 >= screen.sy(52)
        && point.1 <= screen.sy(88)
}

fn recents_scroll_hit(screen: &Screen, point: (i64, i64)) -> Option<i64> {
    let width = screen.width();
    let y = screen.logical_height() - 170;
    if point.1 < y || point.1 > y + 36 {
        return None;
    }
    if point.0 >= 40 && point.0 <= 130 {
        return Some(-1);
    }
    if point.0 >= width - 130 && point.0 <= width - 40 {
        return Some(1);
    }
    None
}

impl AppKind {
    fn label(self) -> &'static str {
        match self {
            AppKind::Music => "Music",
            AppKind::Clock => "Clock",
            AppKind::Settings => "Settings",
            AppKind::Calculator => "Calculator",
            AppKind::Notes => "Notes",
            AppKind::Maps => "Maps",
            AppKind::Photos => "Photos",
            AppKind::Files => "Files",
            AppKind::Weather => "Weather",
            AppKind::Terminal => "Terminal",
            AppKind::Browser => "Browser",
            AppKind::Apk => "APK",
            AppKind::Dvm => "DEX",
            AppKind::Market => "Market",
            AppKind::Monitor => "Monitor",
            AppKind::Plugins => "Plugins",
            AppKind::AiStudio => "AI Studio",
        }
    }
}

fn clock() -> [u64; 7] {
    if let Some(local) = k::local_time() {
        [local[0], local[1], local[2], local[3], local[4], local[5], local[6]]
    } else {
        k::rtc().unwrap_or([0, 2026, 1, 1, 0, 0, 0])
    }
}

fn scale(value: u32, dimension: i64) -> i64 {
    (value as i64 * (dimension - 1)) / 32767
}

fn ease(step: i64, total: i64) -> i64 {
    let progress = (step * 100 / total).clamp(0, 100);
    100 - (100 - progress) * (100 - progress) / 100
}

fn format_time(hour: u64, minute: u64, buffer: &mut [u8; 5]) {
    buffer[0] = b'0' + (hour / 10) as u8;
    buffer[1] = b'0' + (hour % 10) as u8;
    buffer[2] = b':';
    buffer[3] = b'0' + (minute / 10) as u8;
    buffer[4] = b'0' + (minute % 10) as u8;
}

fn format_hms(hour: u64, minute: u64, second: u64, buffer: &mut [u8; 8]) -> usize {
    buffer[0] = b'0' + (hour / 10) as u8;
    buffer[1] = b'0' + (hour % 10) as u8;
    buffer[2] = b':';
    buffer[3] = b'0' + (minute / 10) as u8;
    buffer[4] = b'0' + (minute % 10) as u8;
    buffer[5] = b':';
    buffer[6] = b'0' + (second / 10) as u8;
    buffer[7] = b'0' + (second % 10) as u8;
    8
}

fn format_date(clock: [u64; 7], buffer: &mut [u8; 16]) {
    let (year, month, day) = (clock[1], clock[2].clamp(1, 12), clock[3].clamp(1, 31));
    let weekday = weekday(year, month, day);
    let mut length = 0;
    for byte in WEEKDAYS[weekday].bytes() {
        buffer[length] = byte;
        length += 1;
    }
    buffer[length] = b',';
    buffer[length + 1] = b' ';
    length += 2;
    for byte in MONTHS[(month - 1) as usize].bytes() {
        buffer[length] = byte;
        length += 1;
    }
    buffer[length] = b' ';
    length += 1;
    if day >= 10 {
        buffer[length] = b'0' + (day / 10) as u8;
        length += 1;
    }
    buffer[length] = b'0' + (day % 10) as u8;
    length += 1;
    for byte in &mut buffer[length..16] {
        *byte = 0;
    }
}

const WEEKDAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

fn weekday(year: u64, month: u64, day: u64) -> usize {
    let table = [0u64, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let adjusted = if month < 3 { year - 1 } else { year };
    let index = (adjusted
        + adjusted / 4
        - adjusted / 100
        + adjusted / 400
        + table[(month - 1) as usize]
        + day)
        % 7;
    index as usize
}

fn format_i64(value: i64, buffer: &mut [u8; 20]) -> &str {
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
    core::str::from_utf8(&buffer[index..]).unwrap_or("0")
}

fn str_of(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("")
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}