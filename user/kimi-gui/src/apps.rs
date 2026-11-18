use crate::draw::Screen;
use crate::AppKind;
use libkimi as k;
use crate::theme::{BLACK, SURFACE, SURFACE_HI, SURFACE_MAX, OUTLINE, TEXT, MUTED, RED, RED_DIM, GREEN};


const TERM_WIDTH: usize = 50;
const TERM_LINES: usize = 40;

pub struct Phone {
    digits: [u8; 16],
    len: usize,
    in_call: bool,
    started: u64,
    mute: bool,
    speaker: bool,
}

pub struct Messages {
    open: Option<usize>,
    draft: [u8; 64],
    draft_len: usize,
    sent: [[u8; 64]; 3],
    sent_len: [usize; 3],
    sent_count: usize,
}

pub struct Camera {
    shots: u32,
    front: bool,
    flash: bool,
    status: [u8; 48],
    status_len: usize,
}

pub struct Music {
    track: usize,
    playing: bool,
    elapsed: u64,
    started: u64,
}

pub struct Notes {
    text: [u8; 512],
    len: usize,
    loaded: bool,
    status: [u8; 32],
    status_len: usize,
}

pub struct Maps {
    pin: Option<(i64, i64)>,
}

pub struct Photos {
    selected: Option<usize>,
}

pub struct Files {
    disk: bool,
    count: usize,
    names: [[u8; 28]; 24],
    name_len: [u8; 24],
    paths: [[u8; 48]; 24],
    path_len: [u8; 24],
    sizes: [u64; 24],
    preview: Option<usize>,
    body: [u8; 768],
    body_len: usize,
    binary: bool,
}

pub struct Weather {
    fahrenheit: bool,
    day: usize,
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

pub struct Apps {
    pub phone: Phone,
    pub messages: Messages,
    pub camera: Camera,
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
}

const KEY_LABELS: [&str; 12] = [
    "1", "2", "3", "4", "5", "6", "7", "8", "9", "*", "0", "#",
];

const THREAD_NAMES: [&str; 3] = ["Ada", "Kimi Team", "Rust Weekly"];
const THREAD_TIMES: [&str; 3] = ["now", "2m", "1h"];
const THREAD_PREVIEW: [&str; 3] = [
    "Are we still on for tonight?",
    "Build 0.1 is up to date",
    "This week in bare metal",
];
const THREAD_A: [&str; 6] = [
    "Hey, are we still on for tonight?",
    "Yes! Just finishing the UI.",
    "The new lock screen looks sharp.",
    "Thanks. Now every app works.",
    "Even the terminal?",
    "Even the terminal.",
];
const THREAD_B: [&str; 5] = [
    "KimiOS 0.1 is ready",
    "All apps now render at 1080p.",
    "The kernel console is still there.",
    "Press ESC in the GUI to reach it.",
    "Nice work.",
];
const THREAD_C: [&str; 4] = [
    "This week in bare metal",
    "Rust without an allocator",
    "A tour of the virtio input ring",
    "Plus: writing a tiny compositor",
];
const TRACKS: [(&str, &str); 3] = [
    ("KimiOS Sessions", "Nothing OS / Ambient"),
    ("Bare Metal", "Kimi Sound System"),
    ("Night Shift", "Ambient Works"),
];
const TRACK_MS: [u64; 3] = [215_000, 184_000, 241_000];

fn slice_str(bytes: &[u8]) -> &str {
    core::str::from_utf8(bytes).unwrap_or("")
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

impl Phone {
    fn push(&mut self, digit: u8) {
        if self.len < self.digits.len() {
            self.digits[self.len] = digit;
            self.len += 1;
        }
    }

    fn number(&self) -> &str {
        slice_str(&self.digits[..self.len])
    }

    fn call_seconds(&self) -> u64 {
        if self.in_call {
            let now = k::uptime_ms();
            now.saturating_sub(self.started) / 1000
        } else {
            0
        }
    }
}

impl Music {
    fn elapsed(&self) -> u64 {
        if self.playing {
            self.elapsed + k::uptime_ms().saturating_sub(self.started)
        } else {
            self.elapsed
        }
    }

    fn toggle(&mut self) {
        if self.playing {
            self.elapsed = self.elapsed();
            self.playing = false;
        } else {
            self.started = k::uptime_ms();
            self.playing = true;
        }
    }

    fn select(&mut self, index: usize) {
        self.track = index;
        self.elapsed = 0;
        self.started = k::uptime_ms();
        self.playing = true;
    }

    fn next(&mut self) {
        self.select((self.track + 1) % TRACKS.len());
    }

    fn previous(&mut self) {
        self.select((self.track + TRACKS.len() - 1) % TRACKS.len());
    }
}

pub fn track_info(track: usize) -> (&'static str, &'static str) {
    TRACKS[track % TRACKS.len()]
}

pub fn track_progress(music: &Music) -> i64 {
    let duration = TRACK_MS[music.track % TRACK_MS.len()];
    ((music.elapsed().min(duration) * 1000 / duration) as i64).clamp(0, 1000)
}

pub fn home_music(music: &Music) -> (&'static str, &'static str, bool, i64) {
    let (title, subtitle) = track_info(music.track);
    (title, subtitle, music.playing, track_progress(music))
}

pub fn home_music_toggle(apps: &mut Apps) {
    apps.music.toggle();
}

impl Notes {
    fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Some(handle) = k::open("/disk/notes.txt") {
            let count = k::read_file(handle, 0, &mut self.text);
            k::close(handle);
            self.len = count.min(self.text.len());
        }
        if self.len == 0 {
            let seed = b"KimiOS notes\n\n- bare metal, no allocator\n- 1080p phone UI\n- terminal included\n";
            self.text[..seed.len()].copy_from_slice(seed);
            self.len = seed.len();
        }
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
        let status = if let Some(handle) = k::create("/disk/notes.txt") {
            let written = k::write_file(handle, 0, &self.text[..self.len]);
            k::close(handle);
            if written == self.len {
                b"Saved to /disk/notes.txt" as &[u8]
            } else {
                b"Short write"
            }
        } else {
            b"No disk mounted"
        };
        self.status_len = status.len().min(self.status.len());
        self.status[..self.status_len].copy_from_slice(&status[..self.status_len]);
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
    fn refresh(&mut self) {
        self.count = 0;
        self.preview = None;
        let mut buffer = [0u8; 640];
        let byte_count = k::list(&mut buffer);
        for name in buffer[..byte_count.min(buffer.len())].split(|&byte| byte == 0) {
            if name.is_empty() || self.count == self.names.len() {
                continue;
            }
            let is_disk = name.starts_with(b"/disk/");
            if is_disk != self.disk {
                continue;
            }
            if name.len() > self.paths[self.count].len() {
                continue;
            }
            self.paths[self.count][..name.len()].copy_from_slice(name);
            self.path_len[self.count] = name.len() as u8;
            let shown = if is_disk {
                name.strip_prefix(b"/disk/").unwrap_or(name)
            } else {
                name.strip_prefix(b"/").unwrap_or(name)
            };
            let length = shown.len().min(self.names[self.count].len());
            self.names[self.count][..length].copy_from_slice(&shown[..length]);
            self.name_len[self.count] = length as u8;
            let path = slice_str(&self.paths[self.count][..name.len()]);
            self.sizes[self.count] = k::stat(path).map_or(0, |(_, size)| size);
            self.count += 1;
        }
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
            phone: Phone {
                digits: [0u8; 16],
                len: 0,
                in_call: false,
                started: 0,
                mute: false,
                speaker: false,
            },
            messages: Messages {
                open: None,
                draft: [0u8; 64],
                draft_len: 0,
                sent: [[0u8; 64]; 3],
                sent_len: [0usize; 3],
                sent_count: 0,
            },
            camera: Camera {
                shots: 0,
                front: false,
                flash: false,
                status: [0u8; 48],
                status_len: 0,
            },
            music: Music {
                track: 0,
                playing: true,
                elapsed: 42_000,
                started: k::uptime_ms(),
            },
            notes: Notes {
                text: [0u8; 512],
                len: 0,
                loaded: false,
                status: [0u8; 32],
                status_len: 0,
            },
            maps: Maps { pin: None },
            photos: Photos { selected: None },
            files: Files {
                disk: false,
                count: 0,
                names: [[0u8; 28]; 24],
                name_len: [0u8; 24],
                paths: [[0u8; 48]; 24],
                path_len: [0u8; 24],
                sizes: [0u64; 24],
                preview: None,
                body: [0u8; 768],
                body_len: 0,
                binary: false,
            },
            weather: Weather {
                fahrenheit: false,
                day: 0,
            },
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
        }
    }
}

pub fn open(apps: &mut Apps, app: AppKind) {
    match app {
        AppKind::Notes => apps.notes.load(),
        AppKind::Files => apps.files.refresh(),
        AppKind::Camera => apps.camera.status_len = 0,
        AppKind::Browser => apps.browser.editing = apps.browser.line_count == 0,
        AppKind::Apk => apps.apk.refresh(),
        AppKind::Dvm => apps.dvm.refresh(),
        AppKind::Market => apps.market.refresh(),
        _ => {}
    }
}

pub fn back(apps: &mut Apps, app: AppKind) -> bool {
    match app {
        AppKind::Browser => false,
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
        AppKind::Messages => {
            if apps.messages.open.take().is_some() {
                apps.messages.draft_len = 0;
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
        AppKind::Files => {
            if apps.files.preview.take().is_some() {
                true
            } else {
                false
            }
        }
        AppKind::Phone => {
            if apps.phone.in_call {
                apps.phone.in_call = false;
                true
            } else {
                false
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
        AppKind::Notes | AppKind::Terminal => true,
        AppKind::Messages => apps.messages.open.is_some(),
        AppKind::Browser => apps.browser.editing,
        _ => false,
    }
}

/// Whether an app-space tap lands on the text field itself (reopens the OSK).
pub fn field_hit(apps: &Apps, app: AppKind, point: (i64, i64), osk: bool) -> bool {
    match app {
        AppKind::Notes => point.0 >= 20 && point.0 <= 460 && point.1 >= 160 && point.1 <= 614,
        AppKind::Terminal => point.1 > 96 && point.1 < 770,
        AppKind::Messages => {
            if apps.messages.open.is_none() {
                return false;
            }
            let y = if osk { 392 } else { 690 };
            point.0 >= 20 && point.0 <= 460 && point.1 >= y && point.1 <= y + 56
        }
        AppKind::Browser => point.0 >= 20 && point.0 < 390 && point.1 >= 104 && point.1 <= 148,
        _ => false,
    }
}

pub fn key(apps: &mut Apps, app: AppKind, code: u16, shift: bool) -> bool {
    match app {
        AppKind::Notes => {
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
        AppKind::Messages => {
            if apps.messages.open.is_none() {
                return false;
            }
            match key_event(code, shift) {
                Some((0, true)) => {
                    if apps.messages.draft_len > 0 {
                        apps.messages.draft_len -= 1;
                    }
                    true
                }
                Some((b'\n', _)) => {
                    if apps.messages.draft_len > 0 && apps.messages.sent_count < 3 {
                        let index = apps.messages.sent_count;
                        let length = apps.messages.draft_len.min(64);
                        apps.messages.sent[index][..length]
                            .copy_from_slice(&apps.messages.draft[..length]);
                        apps.messages.sent_len[index] = length;
                        apps.messages.sent_count += 1;
                    }
                    apps.messages.draft_len = 0;
                    true
                }
                Some((byte, _)) => {
                    if apps.messages.draft_len < apps.messages.draft.len() {
                        let index = apps.messages.draft_len;
                        apps.messages.draft[index] = byte;
                        apps.messages.draft_len += 1;
                    }
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
        AppKind::Phone => phone_tap(apps, point),
        AppKind::Messages => messages_tap(apps, point),
        AppKind::Camera => camera_tap(screen, apps, point),
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
        _ => {}
    }
}

fn phone_tap(apps: &mut Apps, point: (i64, i64)) {
    if apps.phone.in_call {
        if point.1 >= 620 && point.1 <= 700 {
            if point.0 >= 180 && point.0 <= 300 {
                apps.phone.in_call = false;
            } else if point.0 < 120 {
                apps.phone.mute = !apps.phone.mute;
            } else if point.0 > 360 {
                apps.phone.speaker = !apps.phone.speaker;
            }
        }
        return;
    }
    if point.1 >= 560 && point.1 <= 626 && point.0 >= 120 && point.0 <= 360 {
        if apps.phone.len > 0 {
            apps.phone.in_call = true;
            apps.phone.started = k::uptime_ms();
            apps.phone.mute = false;
            apps.phone.speaker = false;
        }
        return;
    }
    if point.1 >= 590 && point.1 <= 660 && point.0 >= 388 && point.0 <= 452 {
        apps.phone.len = 0;
        return;
    }
    for row in 0..4i64 {
        for column in 0..3i64 {
            let x = 78 + column * 112;
            let y = 232 + row * 80;
            if point.0 >= x && point.0 < x + 100 && point.1 >= y && point.1 < y + 68 {
                let index = (row * 3 + column) as usize;
                if index < KEY_LABELS.len() {
                    let label = KEY_LABELS[index].as_bytes()[0];
                    apps.phone.push(label);
                }
            }
        }
    }
}

fn messages_tap(apps: &mut Apps, point: (i64, i64)) {
    if apps.messages.open.is_some() {
        return;
    }
    if point.1 >= 130 {
        for index in 0..3 {
            let y = 130 + index as i64 * 104;
            if point.0 >= 20 && point.0 < 460 && point.1 >= y && point.1 < y + 92 {
                apps.messages.open = Some(index);
                apps.messages.draft_len = 0;
                return;
            }
        }
    }
}

fn camera_tap(screen: &Screen, apps: &mut Apps, point: (i64, i64)) {
    let dx = point.0 - 240;
    let dy = point.1 - 690;
    if dx * dx + dy * dy <= 44 * 44 {
        camera_shoot(screen, apps);
        return;
    }
    if point.0 < 160 && point.1 > 650 {
        apps.camera.front = !apps.camera.front;
    } else if point.0 > 320 && point.1 > 650 {
        apps.camera.flash = !apps.camera.flash;
    }
}

fn camera_shoot(screen: &Screen, apps: &mut Apps) {
    let index = apps.camera.shots + 1;
    let mut path = [0u8; 40];
    let prefix = b"/disk/camera-";
    path[..prefix.len()].copy_from_slice(prefix);
    let mut number = [0u8; 20];
    let text = unsigned(index as u64, &mut number);
    path[prefix.len()..prefix.len() + text.len()].copy_from_slice(text.as_bytes());
    let at = prefix.len() + text.len();
    path[at] = b'.';
    path[at + 1] = b'p';
    path[at + 2] = b'p';
    path[at + 3] = b'm';
    let path_text = slice_str(&path[..at + 4]);
    let mut saved = false;
    if let Some(handle) = k::create(path_text) {
        let header = b"P6\n64 114\n255\n";
        let mut image = [0u8; 64 * 114 * 3 + 14];
        image[..header.len()].copy_from_slice(header);
        let mut offset = header.len();
        for y in 0..114i64 {
            for x in 0..64i64 {
                let pixel = screen.pixel(x * 17, y * 17);
                image[offset] = (pixel & 0xff) as u8;
                image[offset + 1] = ((pixel >> 8) & 0xff) as u8;
                image[offset + 2] = ((pixel >> 16) & 0xff) as u8;
                offset += 3;
            }
        }
        let mut written = 0usize;
        let mut ok = true;
        while written < offset {
            let end = (written + 8192).min(offset);
            let count = k::write_file(handle, written, &image[written..end]);
            if count != end - written {
                ok = false;
                break;
            }
            written = end;
        }
        k::close(handle);
        saved = ok;
        if ok {
            apps.camera.shots = index;
        }
    }
    let status: &[u8] = if saved {
        let mut message = [0u8; 48];
        let prefix = b"Saved ";
        message[..prefix.len()].copy_from_slice(prefix);
        let shown = path_text.len().min(40);
        message[prefix.len()..prefix.len() + shown].copy_from_slice(&path[..shown]);
        apps.camera.status = message;
        apps.camera.status_len = prefix.len() + shown;
        b""
    } else {
        b"No disk mounted"
    };
    if !saved {
        apps.camera.status_len = status.len().min(48);
        apps.camera.status[..apps.camera.status_len].copy_from_slice(&status[..apps.camera.status_len]);
    }
}

fn music_tap(apps: &mut Apps, point: (i64, i64)) {
    let dx = point.0 - 240;
    let dy = point.1 - 388;
    if dx * dx + dy * dy <= 42 * 42 {
        apps.music.toggle();
        return;
    }
    let dx = point.0 - 140;
    if dx * dx + dy * dy <= 30 * 30 {
        apps.music.previous();
        return;
    }
    let dx = point.0 - 340;
    if dx * dx + dy * dy <= 30 * 30 {
        apps.music.next();
        return;
    }
    if point.1 >= 470 {
        let index = ((point.1 - 470) / 74) as usize;
        if index < 3 && point.0 >= 20 && point.0 < 460 {
            apps.music.select(index);
        }
    }
}

fn notes_tap(apps: &mut Apps, point: (i64, i64)) {
    if point.0 >= 340 && point.0 <= 456 && point.1 >= 104 && point.1 <= 148 {
        apps.notes.save();
    }
}

fn maps_tap(apps: &mut Apps, point: (i64, i64)) {
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

fn files_tap(apps: &mut Apps, point: (i64, i64)) {
    if apps.files.preview.is_some() {
        apps.files.preview = None;
        return;
    }
    if point.1 >= 100 && point.1 <= 142 {
        if point.0 >= 24 && point.0 < 218 {
            if apps.files.disk {
                apps.files.disk = false;
                apps.files.refresh();
            }
        } else if point.0 >= 230 && point.0 < 456 {
            if !apps.files.disk {
                apps.files.disk = true;
                apps.files.refresh();
            }
        }
        return;
    }
    if point.1 >= 170 {
        let index = ((point.1 - 170) / 30) as usize;
        if index < apps.files.count && point.0 >= 20 && point.0 < 460 {
            apps.files.open_preview(index);
        }
    }
}

fn weather_tap(apps: &mut Apps, point: (i64, i64)) {
    if point.1 >= 104 && point.1 <= 144 && point.0 >= 300 && point.0 <= 456 {
        apps.weather.fahrenheit = !apps.weather.fahrenheit;
        return;
    }
    if point.1 >= 470 {
        let index = ((point.1 - 470) / 64) as usize;
        if index < 5 {
            apps.weather.day = index;
        }
    }
}

pub fn draw(screen: &Screen, apps: &Apps, app: AppKind, clock: [u64; 7], osk: bool) {
    match app {
        AppKind::Phone => draw_phone(screen, &apps.phone),
        AppKind::Messages => draw_messages(screen, &apps.messages, osk),
        AppKind::Camera => draw_camera(screen, &apps.camera),
        AppKind::Music => draw_music(screen, &apps.music),
        AppKind::Notes => draw_notes(screen, &apps.notes, osk),
        AppKind::Maps => draw_maps(screen, &apps.maps),
        AppKind::Photos => draw_photos(screen, &apps.photos, apps.camera.shots),
        AppKind::Files => draw_files(screen, &apps.files),
        AppKind::Weather => draw_weather(screen, &apps.weather, clock),
        AppKind::Terminal => draw_terminal(screen, &apps.terminal, osk),
        AppKind::Browser => draw_browser(screen, &apps.browser, osk),
        AppKind::Apk => draw_apk(screen, &apps.apk),
        AppKind::Dvm => draw_dvm(screen, &apps.dvm),
        AppKind::Market => draw_market(screen, &apps.market),
        _ => {}
    }
}

fn draw_phone(screen: &Screen, phone: &Phone) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    if phone.in_call {
        screen.text_centered(width / 2, 150, phone.number(), 3, TEXT());
        screen.text_centered(width / 2, 200, "In call", 1, MUTED());
        let mut time = [0u8; 8];
        let seconds = phone.call_seconds();
        time[0] = b'0' + (seconds / 600) as u8 % 10;
        time[1] = b'0' + (seconds / 60) as u8 % 10;
        time[2] = b':';
        time[3] = b'0' + (seconds / 10) as u8 % 10;
        time[4] = b'0' + seconds as u8 % 10;
        screen.text_centered(width / 2, 250, slice_str(&time[..5]), 3, TEXT());
        screen.circle(width / 2, 350, 34, SURFACE_MAX());
        screen.circle(width / 2, 350, 18, RED_DIM());
        screen.text_centered(width / 2, 410, "Calling over Wi-Fi", 1, MUTED());
        for (x, label, active) in [
            (84, "Mute", phone.mute),
            (240, "Speaker", phone.speaker),
            (396, "End", false),
        ] {
            let color = if label == "End" { RED() } else if active { RED_DIM() } else { SURFACE_HI() };
            screen.round_rect(x - 40, 620, 80, 80, 40, color);
            screen.text_centered(x, 654, label, 1, TEXT());
        }
        return;
    }
    screen.round_rect(20, 112, width - 40, 88, 28, SURFACE());
    let number = phone.number();
    if number.is_empty() {
        screen.text_centered(width / 2, 138, "Enter number", 2, MUTED());
    } else {
        screen.text_centered(width / 2, 134, number, 3, TEXT());
    }
    for row in 0..4i64 {
        for column in 0..3i64 {
            let x = 78 + column * 112;
            let y = 232 + row * 80;
            let index = (row * 3 + column) as usize;
            let label = KEY_LABELS[index];
            screen.circle(x + 50, y + 34, 32, SURFACE());
            screen.text_centered(x + 50, y + 22, label, 3, TEXT());
        }
    }
    button(screen, 120, 560, 240, 66, "Call", GREEN(), BLACK());
    screen.round_rect(388, 590, 64, 32, 16, SURFACE());
    screen.text_centered(420, 598, "DEL", 1, MUTED());
}

fn draw_messages(screen: &Screen, messages: &Messages, osk: bool) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    match messages.open {
        None => {
            for index in 0..3 {
                let y = 130 + index as i64 * 104;
                screen.round_rect(20, y, width - 40, 92, 26, SURFACE());
                screen.circle(58, y + 46, 22, if index == 0 { RED_DIM() } else { SURFACE_MAX() });
                screen.text(58, y + 36, &THREAD_NAMES[index][..1], 2, if index == 0 { RED() } else { TEXT() });
                screen.text(94, y + 20, THREAD_NAMES[index], 2, TEXT());
                screen.text(94, y + 48, THREAD_PREVIEW[index], 1, MUTED());
                screen.text(width - 28 - Screen::text_width(THREAD_TIMES[index], 1), y + 22, THREAD_TIMES[index], 1, MUTED());
                if index == 0 {
                    screen.circle(width - 36, y + 62, 5, RED());
                }
            }
        }
        Some(index) => {
            screen.text(24, 106, THREAD_NAMES[index], 2, TEXT());
            screen.text(24, 132, "online", 1, MUTED());
            let mut y = 164i64;
            let thread: &[&str] = match index {
                0 => &THREAD_A,
                1 => &THREAD_B,
                _ => &THREAD_C,
            };
            let draft_y = if osk { 392 } else { 690 };
            for (order, text) in thread.iter().enumerate() {
                if osk && order + 2 < thread.len() {
                    continue;
                }
                if osk && y > draft_y - 72 {
                    break;
                }
                let mine = order % 2 == 1;
                y = bubble(screen, text, y, mine);
            }
            for order in 0..messages.sent_count {
                if osk && y > draft_y - 72 {
                    break;
                }
                y = bubble(screen, slice_str(&messages.sent[order][..messages.sent_len[order]]), y, true);
            }
            screen.round_rect(20, draft_y, width - 40, 56, 28, SURFACE());
            if messages.draft_len == 0 {
                screen.text(38, draft_y + 18, "Type a message and press Enter", 1, MUTED());
            } else {
                screen.text(38, draft_y + 16, slice_str(&messages.draft[..messages.draft_len.min(20)]), 2, TEXT());
            }
        }
    }
}

fn bubble(screen: &Screen, text: &str, y: i64, mine: bool) -> i64 {
    let width = screen.width();
    let bytes = text.as_bytes();
    let mut ranges = [(0usize, 0usize); 3];
    let mut lines = 0usize;
    let mut start = 0usize;
    while start < bytes.len() && lines < 3 {
        let mut end = (start + 18).min(bytes.len());
        if end < bytes.len() {
            if let Some(space) = bytes[start..end].iter().rposition(|&byte| byte == b' ') {
                if space > 0 {
                    end = start + space;
                }
            }
        }
        ranges[lines] = (start, end);
        lines += 1;
        start = end;
        while start < bytes.len() && bytes[start] == b' ' {
            start += 1;
        }
    }
    let lines = lines.max(1);
    let height = 18 + lines as i64 * 24;
    let mut widest = 0i64;
    for index in 0..lines {
        let (start, end) = ranges[index];
        widest = widest.max(Screen::text_width(slice_str(&bytes[start..end]), 2));
    }
    let bubble_width = (widest + 32).clamp(80, 360).min(width - 48);
    let x = if mine { width - 24 - bubble_width } else { 24 };
    let color = if mine { RED_DIM() } else { SURFACE_HI() };
    screen.round_rect(x, y, bubble_width, height, 20, color);
    for index in 0..lines {
        let (start, end) = ranges[index];
        screen.text(x + 16, y + 14 + index as i64 * 24, slice_str(&bytes[start..end]), 2, TEXT());
    }
    y + height + 12
}

fn draw_camera(screen: &Screen, camera: &Camera) {
    let width = screen.width();
    let top = if camera.front { 0x0020_2430 } else { 0x0010_1c2c };
    let bottom = if camera.front { 0x0060_5060 } else { 0x00d0_7040 };
    screen.gradient(0, 93, width, 527, top, bottom);
    screen.circle(if camera.front { 100 } else { 360 }, 210, 58, 0x00f0_d080);
    screen.circle(140, 520, 210, 0x0040_6048);
    screen.circle(400, 560, 260, 0x0030_4838);
    screen.circle(240, 620, 330, 0x0028_3830);
    for index in 1..3 {
        let x = width * index / 3;
        screen.fill(x, 93, 1, 527, 0x40ff_ffff);
        let y = 93 + 527 * index / 3;
        screen.fill(0, y, width, 1, 0x40ff_ffff);
    }
    screen.round_rect(18, 106, 86, 28, 14, 0x8000_0000);
    screen.text(30, 114, if camera.front { "FRONT" } else { "12MP" }, 1, TEXT());
    screen.round_rect(width - 104, 106, 86, 28, 14, 0x8000_0000);
    screen.text(width - 92, 114, if camera.flash { "FLASH" } else { "AUTO" }, 1, TEXT());
    screen.circle(240, 690, 38, SURFACE_MAX());
    screen.ring(240, 690, 38, 3, TEXT());
    screen.circle(240, 690, 28, TEXT());
    screen.round_rect(84, 668, 44, 44, 22, SURFACE_HI());
    screen.text_centered(106, 680, "F", 2, TEXT());
    screen.round_rect(352, 668, 44, 44, 22, SURFACE_HI());
    screen.text_centered(374, 680, "L", 2, TEXT());
    if camera.status_len > 0 {
        screen.text_centered(240, 748, slice_str(&camera.status[..camera.status_len]), 1, GREEN());
    } else {
        screen.text_centered(240, 748, "Tap the shutter to save a PPM", 1, MUTED());
    }
}

fn draw_music(screen: &Screen, music: &Music) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    screen.gradient(40, 120, 160, 160, 0x0030_2028, RED_DIM());
    screen.circle(120, 200, 46, 0x0010_0c10);
    screen.ring(120, 200, 46, 2, RED());
    screen.text(40, 300, track_info(music.track).0, 3, TEXT());
    screen.text(40, 336, track_info(music.track).1, 1, MUTED());
    screen.round_rect(40, 380, width - 80, 8, 4, OUTLINE());
    let progress = track_progress(music);
    screen.round_rect(40, 380, (width - 80) * progress / 1000, 8, 4, RED());
    let elapsed = music.elapsed().min(TRACK_MS[music.track % 3]) / 1000;
    let total = TRACK_MS[music.track % 3] / 1000;
    let mut time = [0u8; 20];
    let left = unsigned(elapsed, &mut time);
    screen.text(40, 400, left, 1, MUTED());
    let mut time2 = [0u8; 20];
    let right = unsigned(total, &mut time2);
    screen.text(width - 40 - Screen::text_width(right, 1), 400, right, 1, MUTED());
    screen.circle(140, 470, 28, SURFACE_HI());
    screen.line(148, 458, 148, 482, 3, TEXT());
    screen.line(148, 458, 132, 470, 3, TEXT());
    screen.line(148, 482, 132, 470, 3, TEXT());
    draw_play_button(screen, 240, 470, music.playing);
    screen.circle(340, 470, 28, SURFACE_HI());
    screen.line(332, 458, 332, 482, 3, TEXT());
    screen.line(332, 458, 348, 470, 3, TEXT());
    screen.line(332, 482, 348, 470, 3, TEXT());
    screen.text(24, 530, "UP NEXT", 1, MUTED());
    for index in 0..TRACKS.len() {
        let y = 552 + index as i64 * 74;
        let active = index == music.track;
        screen.round_rect(20, y, width - 40, 64, 22, if active { SURFACE_HI() } else { SURFACE() });
        screen.text(40, y + 14, TRACKS[index].0, 2, if active { TEXT() } else { MUTED() });
        screen.text(40, y + 40, TRACKS[index].1, 1, if active { RED() } else { MUTED() });
        if active {
            screen.circle(width - 46, y + 32, 6, RED());
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
    screen.text(24, 110, "notes.txt", 2, TEXT());
    button(screen, 340, 104, 116, 44, "SAVE", RED(), TEXT());
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
        screen.text(24, status_y, "Typed text is saved with SAVE", 1, MUTED());
    }
    if !osk {
        screen.text(24, 654, "Enter adds a line, backspace deletes", 1, MUTED());
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
    if let Some((px, py)) = maps.pin {
        screen.text(40, 654, "Pinned location", 2, TEXT());
        let lat = 377_749 + (620 - py) * 3;
        let lon = -1_224_194 + (px - 20) * 3;
        let mut line = [0u8; 48];
        let mut used = 0;
        for (value, negative) in [(lat, false), (lon, true)] {
            if negative {
                line[used] = b'-';
                used += 1;
            }
            let whole = value.unsigned_abs() / 10_000;
            let fraction = value.unsigned_abs() % 10_000;
            let mut buf = [0u8; 20];
            let text = unsigned(whole, &mut buf);
            for &byte in text.as_bytes() {
                line[used] = byte;
                used += 1;
            }
            line[used] = b'.';
            used += 1;
            for digit in 0..4 {
                let divisor = 10u64.pow(3 - digit);
                line[used] = b'0' + ((fraction / divisor) % 10) as u8;
                used += 1;
            }
            line[used] = b' ';
            used += 1;
            line[used] = b' ';
            used += 1;
        }
        screen.text(40, 688, slice_str(&line[..used]), 1, MUTED());
    } else {
        screen.text(40, 654, "Maps preview", 2, TEXT());
        screen.text(40, 684, "Local, offline and tiny", 1, MUTED());
    }
}

fn draw_photos(screen: &Screen, photos: &Photos, shots: u32) {
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
    let mut buf = [0u8; 20];
    let shots_text = unsigned(shots as u64, &mut buf);
    screen.text(24, 620, "Camera shots:", 1, MUTED());
    screen.text(156, 620, shots_text, 1, TEXT());
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

fn draw_files(screen: &Screen, files: &Files) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
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
        screen.text(40, 642, "Tap to close", 1, MUTED());
        return;
    }
    let tab = if files.disk { "DISK" } else { "RAMFS" };
    screen.round_rect(24, 100, 194, 42, 21, if files.disk { SURFACE() } else { RED_DIM() });
    screen.text_centered(121, 110, "RAMFS", 2, if files.disk { MUTED() } else { TEXT() });
    screen.round_rect(230, 100, 226, 42, 21, if files.disk { RED_DIM() } else { SURFACE() });
    screen.text_centered(343, 110, "DISK", 2, if files.disk { TEXT() } else { MUTED() });
    for index in 0..files.count.min(18) {
        let y = 170 + index as i64 * 30;
        screen.round_rect(20, y, width - 40, 28, 14, if index % 2 == 0 { SURFACE() } else { BLACK() });
        screen.round_rect(34, y + 7, 14, 14, 3, MUTED());
        let label = slice_str(&files.names[index][..files.name_len[index] as usize]);
        screen.text(60, y + 10, label, 1, TEXT());
        let mut buf = [0u8; 20];
        let size = unsigned(files.sizes[index], &mut buf);
        screen.text(width - 28 - Screen::text_width(size, 1), y + 10, size, 1, MUTED());
    }
    screen.text(24, 730, tab, 1, MUTED());
}

fn draw_weather(screen: &Screen, weather: &Weather, clock: [u64; 7]) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());
    screen.round_rect(20, 104, width - 40, 240, 30, SURFACE());
    let temp = if weather.fahrenheit { 72 } else { 22 };
    let _ = temp;
    let mut buf = [0u8; 20];
    let text = signed(temp, &mut buf);
    screen.text(40, 132, text, 5, TEXT());
    screen.text(40 + Screen::text_width(text, 5) + 8, 140, if weather.fahrenheit { "F" } else { "C" }, 2, MUTED());
    screen.text(40, 220, "Clear", 2, TEXT());
    screen.text(40, 252, if weather.fahrenheit { "Feels like 75 F" } else { "Feels like 24 C" }, 1, MUTED());
    screen.circle(392, 248, 34, 0x00f0_c050);
    for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0), (1, 1), (-1, 1), (1, -1), (-1, -1)] {
        screen.line(392 + dx * 42, 248 + dy * 42, 392 + dx * 56, 248 + dy * 56, 2, 0x00f0_c050);
    }
    button(screen, 304, 104, 152, 40, if weather.fahrenheit { "USE C" } else { "USE F" }, SURFACE_HI(), MUTED());
    screen.text(24, 370, "5-DAY FORECAST", 1, MUTED());
    let days = ["MON", "TUE", "WED", "THU", "FRI"];
    let highs = [24i64, 26, 23, 21, 25];
    let lows = [15i64, 17, 14, 12, 16];
    for index in 0..5 {
        let y = 398 + index as i64 * 64;
        let active = index == weather.day;
        screen.round_rect(20, y, width - 40, 54, 18, if active { SURFACE_HI() } else { SURFACE() });
        screen.text(40, y + 18, days[index], 2, if active { TEXT() } else { MUTED() });
        let bar = (highs[index] - 10) * 12;
        screen.round_rect(160, y + 24, bar, 6, 3, RED());
        screen.round_rect(160, y + 24, (lows[index] - 10) * 12, 6, 3, OUTLINE());
        let mut hi = [0u8; 20];
        let mut lo = [0u8; 20];
        let high = signed(if weather.fahrenheit { highs[index] * 9 / 5 + 32 } else { highs[index] }, &mut hi);
        let low = signed(if weather.fahrenheit { lows[index] * 9 / 5 + 32 } else { lows[index] }, &mut lo);
        screen.text(width - 108, y + 18, high, 2, TEXT());
        screen.text(width - 48, y + 18, low, 2, MUTED());
    }
    let _ = clock;
    screen.text(24, 730, "Updated from the RTC", 1, MUTED());
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
        browser.editing = true;
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
    browser.scroll = 0;
    browser.line_count = 0;
    browser.title_len = 0;

    let mut url_buf = [0u8; 128];
    let url_len = browser.url_len.min(128);
    url_buf[..url_len].copy_from_slice(&browser.url[..url_len]);
    let url = &url_buf[..url_len];
    let url = if url.starts_with(b"http://") { &url[7..] } else { url };
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

fn draw_browser(screen: &Screen, browser: &Browser, osk: bool) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK());

    screen.round_rect(20, 104, width - 110, 44, 22, SURFACE_HI());
    if browser.url_len == 0 {
        screen.text(36, 118, "enter address", 1, 0x0050_505a);
    } else {
        let shown = browser.url_len.min(40);
        screen.text(36, 118, slice_str(&browser.url[..shown]), 1, if browser.editing { TEXT() } else { MUTED() });
    }
    screen.round_rect(width - 84, 104, 64, 44, 22, RED());
    screen.text_centered(width - 52, 116, "GO", 2, TEXT());

    if browser.title_len > 0 {
        let shown = browser.title_len.min(44);
        screen.text(20, 162, slice_str(&browser.title[..shown]), 2, TEXT());
    }

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
            screen.text_centered(width / 2, 416, "e.g. example.com", 1, 0x0050_505a);
        }
    }

    let scroll_y = if osk { 410 } else { 700 };
    screen.round_rect(width - 110, scroll_y, 44, 40, 12, SURFACE_HI());
    screen.text_centered(width - 88, scroll_y + 10, "up", 1, TEXT());
    screen.round_rect(width - 60, scroll_y, 44, 40, 12, SURFACE_HI());
    screen.text_centered(width - 38, scroll_y + 10, "dn", 1, TEXT());
    screen.text(20, scroll_y + 12, slice_str(&browser.status[..browser.status_len]), 1, MUTED());
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

// ---- Marketplace (powered by kpkg) ----------------------------------------

#[derive(Clone, Copy)]
struct MarketItem {
    name: [u8; 32],
    name_len: usize,
    desc: [u8; 64],
    desc_len: usize,
    kind: u8,    // 0 theme, 1 font, 2 app
    source: u8,  // 0 built-in, 1 repo
    index: usize,
    installed: bool,
}

impl MarketItem {
    fn name_str(&self) -> &str { slice_str(&self.name[..self.name_len]) }
    fn desc_str(&self) -> &str { slice_str(&self.desc[..self.desc_len]) }
}

pub struct Market {
    tab: usize,
    scroll: [usize; 3],
    items: [MarketItem; 40],
    count: usize,
    status: [u8; 64],
    status_len: usize,
    repo_ok: bool,
}

const EMPTY_ITEM: MarketItem = MarketItem {
    name: [0; 32], name_len: 0, desc: [0; 64], desc_len: 0,
    kind: 0, source: 0, index: 0, installed: false,
};

impl Market {
    pub fn new() -> Self {
        let mut m = Self {
            tab: 0,
            scroll: [0; 3],
            items: [EMPTY_ITEM; 40],
            count: 0,
            status: [0; 64],
            status_len: 0,
            repo_ok: false,
        };
        m.set_status("Themes, fonts & apps — powered by kpkg");
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
        self.repo_ok = k::kpkg::repo_cached();
        for t in 0..3 {
            let n = self.kind_count(t as u8);
            if self.scroll[t] + 6 > n { self.scroll[t] = n.saturating_sub(6); }
        }
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
                crate::theme::save();
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
                crate::theme::save();
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
        m.tab = if point.0 < 165 { 0 } else if point.0 < 315 { 1 } else { 2 };
        return;
    }
    if point.1 >= 150 && point.1 <= 186 && point.0 >= 330 {
        if k::kpkg::update_quiet("") {
            m.set_status("Repo updated");
        } else {
            m.set_status("Fetch failed — built-ins still available");
        }
        m.refresh();
        return;
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

    let tabs = ["Themes", "Fonts", "Apps"];
    for t in 0..3 {
        let x = 20 + t as i64 * 150;
        let w = if t == 2 { 136 } else { 140 };
        let active = m.tab == t;
        screen.round_rect(x, 100, w, 42, 21, if active { RED_DIM() } else { SURFACE() });
        screen.text_centered(x + w / 2, 110, tabs[t], 2, if active { TEXT() } else { MUTED() });
    }

    screen.text(24, 162, slice_str(&m.status[..m.status_len]), 1, MUTED());
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
        let tag = match it.kind { 0 => "THEME", 1 => "FONT", _ => "APP" };
        screen.text(40, y + 66, tag, 1, OUTLINE());

        let active = item_active(&it);
        let (label, bg, fg) = if it.kind == 2 {
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
