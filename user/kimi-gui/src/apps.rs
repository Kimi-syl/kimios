use crate::draw::Screen;
use crate::AppKind;
use libkimi as k;

const BLACK: u32 = 0x0000_0000;
const SURFACE: u32 = 0x0014_1419;
const SURFACE_HI: u32 = 0x001e_1e25;
const SURFACE_MAX: u32 = 0x0027_272f;
const OUTLINE: u32 = 0x0035_3540;
const TEXT: u32 = 0x00f2_f2f6;
const MUTED: u32 = 0x0093_93a0;
const RED: u32 = 0x00e0_352b;
const RED_DIM: u32 = 0x005c_1a16;
const GREEN: u32 = 0x0060_d18a;

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
        }
    }
}

pub fn open(apps: &mut Apps, app: AppKind) {
    match app {
        AppKind::Notes => apps.notes.load(),
        AppKind::Files => apps.files.refresh(),
        AppKind::Camera => apps.camera.status_len = 0,
        _ => {}
    }
}

pub fn back(apps: &mut Apps, app: AppKind) -> bool {
    match app {
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
    let (name, rest) = line.split_at(split);
    let argument = rest.trim();
    match name {
        "help" => {
            apps.terminal.push("help  ls  cat  echo  date  df  mem  ps");
            apps.terminal.push("uname  uptime  clear  exit");
        }
        "ls" => {
            let mut buffer = [0u8; 640];
            let popped = if argument == "/disk" {
                k::listdir("/disk", &mut buffer).unwrap_or(0)
            } else {
                k::list(&mut buffer)
            };
            let mut count = 0;
            for entry in buffer[..popped.min(buffer.len())].split(|&byte| byte == 0) {
                if entry.is_empty() {
                    continue;
                }
                apps.terminal.push(slice_str(entry));
                count += 1;
                if count == 16 {
                    break;
                }
            }
        }
        "cat" => {
            let path = if argument.starts_with('/') {
                argument
            } else {
                apps.terminal.push("usage: cat /file");
                return;
            };
            match k::open(path) {
                Some(handle) => {
                    let mut body = [0u8; 512];
                    let count = k::read_file(handle, 0, &mut body);
                    k::close(handle);
                    body[..count].iter_mut().for_each(|byte| {
                        if *byte == b'\r' || (*byte < 32 && *byte != b'\n' && *byte != b'\t') || *byte > 126 {
                            *byte = b'.';
                        }
                    });
                    apps.terminal.push(slice_str(&body[..count]));
                }
                None => apps.terminal.push("cat: file not found"),
            }
        }
        "echo" => apps.terminal.push(argument),
        "date" => {
            let clock = k::rtc().unwrap_or([0, 2026, 1, 1, 0, 0, 0]);
            let mut buf = [0u8; 20];
            let text = format_date(clock, &mut buf);
            apps.terminal.push(text);
        }
        "df" => {
            if let Some(info) = k::diskinfo() {
                let mut buf = [0u8; 20];
                let mut buf2 = [0u8; 20];
                let sectors = unsigned(info >> 32, &mut buf);
                let files = unsigned(info & 0xffff_ffff, &mut buf2);
                let mut line = [0u8; 64];
                let mut used = 0;
                for part in ["sectors ", sectors, " files ", files] {
                    for &byte in part.as_bytes() {
                        if used == line.len() {
                            break;
                        }
                        line[used] = byte;
                        used += 1;
                    }
                }
                apps.terminal.push(slice_str(&line[..used]));
                if let Some(free) = k::free_blocks() {
                    let mut line = [0u8; 40];
                    let prefix = b"free blocks ";
                    line[..prefix.len()].copy_from_slice(prefix);
                    let mut buf2 = [0u8; 20];
                    let value = unsigned(free, &mut buf2).as_bytes();
                    line[prefix.len()..prefix.len() + value.len()].copy_from_slice(value);
                    apps.terminal.push(slice_str(&line[..prefix.len() + value.len()]));
                }
            } else {
                apps.terminal.push("df: no disk mounted");
            }
        }
        "mem" => {
            let mut buf = [0u8; 20];
            let free = unsigned(k::free_frames(), &mut buf);
            let mut line = [0u8; 40];
            let prefix = b"free frames ";
            line[..prefix.len()].copy_from_slice(prefix);
            line[prefix.len()..prefix.len() + free.len()].copy_from_slice(free.as_bytes());
            apps.terminal.push(slice_str(&line[..prefix.len() + free.len()]));
        }
        "ps" => {
            let mut rows = [0u64; 64];
            let count = k::ps(&mut rows);
            apps.terminal.push("PID PPID STATE EXIT");
            for index in 0..count.min(rows.len() / 4) {
                let base = index * 4;
                let state = match rows[base + 2] {
                    0 => "free",
                    1 => "ready",
                    2 => "run",
                    3 => "block",
                    _ => "dead",
                };
                let mut line = [0u8; 40];
                let mut used = 0;
                for piece in [
                    unsigned(rows[base], &mut [0u8; 20]),
                    unsigned(rows[base + 1], &mut [0u8; 20]),
                    state,
                ] {
                    for &byte in piece.as_bytes() {
                        if used == line.len() {
                            break;
                        }
                        line[used] = byte;
                        used += 1;
                    }
                    if used < line.len() {
                        line[used] = b' ';
                        used += 1;
                    }
                }
                apps.terminal.push(slice_str(&line[..used]));
            }
        }
        "uname" => apps.terminal.push("KimiOS 0.1 aarch64 bare metal"),
        "uptime" => {
            let mut buf = [0u8; 20];
            let seconds = k::uptime_ms() / 1000;
            let mut line = [0u8; 32];
            let prefix = b"up ";
            line[..prefix.len()].copy_from_slice(prefix);
            let value = unsigned(seconds, &mut buf);
            line[prefix.len()..prefix.len() + value.len()].copy_from_slice(value.as_bytes());
            let suffix = b" seconds";
            let at = prefix.len() + value.len();
            line[at..at + suffix.len()].copy_from_slice(suffix);
            apps.terminal.push(slice_str(&line[..at + suffix.len()]));
        }
        "clear" => apps.terminal.clear(),
        "exit" => apps.terminal.push("Close with the back arrow."),
        _ => {
            let mut message = [0u8; 72];
            let prefix = b"not found: ";
            message[..prefix.len()].copy_from_slice(prefix);
            let shown = name.len().min(48);
            message[prefix.len()..prefix.len() + shown].copy_from_slice(&name.as_bytes()[..shown]);
            apps.terminal.push(slice_str(&message[..prefix.len() + shown]));
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

pub fn tap(screen: &Screen, apps: &mut Apps, app: AppKind, point: (i64, i64), _clock: [u64; 7]) {
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

pub fn draw(screen: &Screen, apps: &Apps, app: AppKind, clock: [u64; 7]) {
    match app {
        AppKind::Phone => draw_phone(screen, &apps.phone),
        AppKind::Messages => draw_messages(screen, &apps.messages),
        AppKind::Camera => draw_camera(screen, &apps.camera),
        AppKind::Music => draw_music(screen, &apps.music),
        AppKind::Notes => draw_notes(screen, &apps.notes),
        AppKind::Maps => draw_maps(screen, &apps.maps),
        AppKind::Photos => draw_photos(screen, &apps.photos, apps.camera.shots),
        AppKind::Files => draw_files(screen, &apps.files),
        AppKind::Weather => draw_weather(screen, &apps.weather, clock),
        AppKind::Terminal => draw_terminal(screen, &apps.terminal),
        _ => {}
    }
}

fn draw_phone(screen: &Screen, phone: &Phone) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
    if phone.in_call {
        screen.text_centered(width / 2, 150, phone.number(), 3, TEXT);
        screen.text_centered(width / 2, 200, "In call", 1, MUTED);
        let mut time = [0u8; 8];
        let seconds = phone.call_seconds();
        time[0] = b'0' + (seconds / 600) as u8 % 10;
        time[1] = b'0' + (seconds / 60) as u8 % 10;
        time[2] = b':';
        time[3] = b'0' + (seconds / 10) as u8 % 10;
        time[4] = b'0' + seconds as u8 % 10;
        screen.text_centered(width / 2, 250, slice_str(&time[..5]), 3, TEXT);
        screen.circle(width / 2, 350, 34, SURFACE_MAX);
        screen.circle(width / 2, 350, 18, RED_DIM);
        screen.text_centered(width / 2, 410, "Calling over Wi-Fi", 1, MUTED);
        for (x, label, active) in [
            (84, "Mute", phone.mute),
            (240, "Speaker", phone.speaker),
            (396, "End", false),
        ] {
            let color = if label == "End" { RED } else if active { RED_DIM } else { SURFACE_HI };
            screen.round_rect(x - 40, 620, 80, 80, 40, color);
            screen.text_centered(x, 654, label, 1, TEXT);
        }
        return;
    }
    screen.round_rect(20, 112, width - 40, 88, 28, SURFACE);
    let number = phone.number();
    if number.is_empty() {
        screen.text_centered(width / 2, 138, "Enter number", 2, MUTED);
    } else {
        screen.text_centered(width / 2, 134, number, 3, TEXT);
    }
    for row in 0..4i64 {
        for column in 0..3i64 {
            let x = 78 + column * 112;
            let y = 232 + row * 80;
            let index = (row * 3 + column) as usize;
            let label = KEY_LABELS[index];
            screen.circle(x + 50, y + 34, 32, SURFACE);
            screen.text_centered(x + 50, y + 22, label, 3, TEXT);
        }
    }
    button(screen, 120, 560, 240, 66, "Call", GREEN, BLACK);
    screen.round_rect(388, 590, 64, 32, 16, SURFACE);
    screen.text_centered(420, 598, "DEL", 1, MUTED);
}

fn draw_messages(screen: &Screen, messages: &Messages) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
    match messages.open {
        None => {
            for index in 0..3 {
                let y = 130 + index as i64 * 104;
                screen.round_rect(20, y, width - 40, 92, 26, SURFACE);
                screen.circle(58, y + 46, 22, if index == 0 { RED_DIM } else { SURFACE_MAX });
                screen.text(58, y + 36, &THREAD_NAMES[index][..1], 2, if index == 0 { RED } else { TEXT });
                screen.text(94, y + 20, THREAD_NAMES[index], 2, TEXT);
                screen.text(94, y + 48, THREAD_PREVIEW[index], 1, MUTED);
                screen.text(width - 28 - Screen::text_width(THREAD_TIMES[index], 1), y + 22, THREAD_TIMES[index], 1, MUTED);
                if index == 0 {
                    screen.circle(width - 36, y + 62, 5, RED);
                }
            }
        }
        Some(index) => {
            screen.text(24, 106, THREAD_NAMES[index], 2, TEXT);
            screen.text(24, 132, "online", 1, MUTED);
            let mut y = 164i64;
            let thread: &[&str] = match index {
                0 => &THREAD_A,
                1 => &THREAD_B,
                _ => &THREAD_C,
            };
            for (order, text) in thread.iter().enumerate() {
                let mine = order % 2 == 1;
                y = bubble(screen, text, y, mine);
            }
            for order in 0..messages.sent_count {
                y = bubble(screen, slice_str(&messages.sent[order][..messages.sent_len[order]]), y, true);
            }
            screen.round_rect(20, 690, width - 40, 56, 28, SURFACE);
            if messages.draft_len == 0 {
                screen.text(38, 708, "Type a message and press Enter", 1, MUTED);
            } else {
                screen.text(38, 706, slice_str(&messages.draft[..messages.draft_len.min(20)]), 2, TEXT);
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
    let color = if mine { RED_DIM } else { SURFACE_HI };
    screen.round_rect(x, y, bubble_width, height, 20, color);
    for index in 0..lines {
        let (start, end) = ranges[index];
        screen.text(x + 16, y + 14 + index as i64 * 24, slice_str(&bytes[start..end]), 2, TEXT);
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
    screen.text(30, 114, if camera.front { "FRONT" } else { "12MP" }, 1, TEXT);
    screen.round_rect(width - 104, 106, 86, 28, 14, 0x8000_0000);
    screen.text(width - 92, 114, if camera.flash { "FLASH" } else { "AUTO" }, 1, TEXT);
    screen.circle(240, 690, 38, SURFACE_MAX);
    screen.ring(240, 690, 38, 3, TEXT);
    screen.circle(240, 690, 28, TEXT);
    screen.round_rect(84, 668, 44, 44, 22, SURFACE_HI);
    screen.text_centered(106, 680, "F", 2, TEXT);
    screen.round_rect(352, 668, 44, 44, 22, SURFACE_HI);
    screen.text_centered(374, 680, "L", 2, TEXT);
    if camera.status_len > 0 {
        screen.text_centered(240, 748, slice_str(&camera.status[..camera.status_len]), 1, GREEN);
    } else {
        screen.text_centered(240, 748, "Tap the shutter to save a PPM", 1, MUTED);
    }
}

fn draw_music(screen: &Screen, music: &Music) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
    screen.gradient(40, 120, 160, 160, 0x0030_2028, RED_DIM);
    screen.circle(120, 200, 46, 0x0010_0c10);
    screen.ring(120, 200, 46, 2, RED);
    screen.text(40, 300, track_info(music.track).0, 3, TEXT);
    screen.text(40, 336, track_info(music.track).1, 1, MUTED);
    screen.round_rect(40, 380, width - 80, 8, 4, OUTLINE);
    let progress = track_progress(music);
    screen.round_rect(40, 380, (width - 80) * progress / 1000, 8, 4, RED);
    let elapsed = music.elapsed().min(TRACK_MS[music.track % 3]) / 1000;
    let total = TRACK_MS[music.track % 3] / 1000;
    let mut time = [0u8; 20];
    let left = unsigned(elapsed, &mut time);
    screen.text(40, 400, left, 1, MUTED);
    let mut time2 = [0u8; 20];
    let right = unsigned(total, &mut time2);
    screen.text(width - 40 - Screen::text_width(right, 1), 400, right, 1, MUTED);
    screen.circle(140, 470, 28, SURFACE_HI);
    screen.line(148, 458, 148, 482, 3, TEXT);
    screen.line(148, 458, 132, 470, 3, TEXT);
    screen.line(148, 482, 132, 470, 3, TEXT);
    draw_play_button(screen, 240, 470, music.playing);
    screen.circle(340, 470, 28, SURFACE_HI);
    screen.line(332, 458, 332, 482, 3, TEXT);
    screen.line(332, 458, 348, 470, 3, TEXT);
    screen.line(332, 482, 348, 470, 3, TEXT);
    screen.text(24, 530, "UP NEXT", 1, MUTED);
    for index in 0..TRACKS.len() {
        let y = 552 + index as i64 * 74;
        let active = index == music.track;
        screen.round_rect(20, y, width - 40, 64, 22, if active { SURFACE_HI } else { SURFACE });
        screen.text(40, y + 14, TRACKS[index].0, 2, if active { TEXT } else { MUTED });
        screen.text(40, y + 40, TRACKS[index].1, 1, if active { RED } else { MUTED });
        if active {
            screen.circle(width - 46, y + 32, 6, RED);
        }
    }
}

fn draw_play_button(screen: &Screen, cx: i64, cy: i64, playing: bool) {
    screen.circle(cx, cy, 38, RED);
    if playing {
        screen.fill(cx - 10, cy - 13, 6, 26, TEXT);
        screen.fill(cx + 4, cy - 13, 6, 26, TEXT);
    } else {
        screen.line(cx - 8, cy - 14, cx - 8, cy + 14, 4, TEXT);
        screen.line(cx - 8, cy - 14, cx + 14, cy, 4, TEXT);
        screen.line(cx - 8, cy + 14, cx + 14, cy, 4, TEXT);
    }
}

fn draw_notes(screen: &Screen, notes: &Notes) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
    screen.text(24, 110, "notes.txt", 2, TEXT);
    button(screen, 340, 104, 116, 44, "SAVE", RED, TEXT);
    screen.round_rect(20, 160, width - 40, 454, 26, SURFACE);
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
        if y > 580 {
            break;
        }
        let ch = [byte];
        screen.text(x, y, slice_str(&ch), 1, TEXT);
        x += 8;
        line_length += 1;
    }
    if notes.status_len > 0 {
        screen.text(24, 626, slice_str(&notes.status[..notes.status_len]), 1, GREEN);
    } else {
        screen.text(24, 626, "Typed text is saved with SAVE", 1, MUTED);
    }
    screen.text(24, 654, "Enter adds a line, backspace deletes", 1, MUTED);
}

fn draw_maps(screen: &Screen, maps: &Maps) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
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
    screen.text(56, 420, "Park", 1, GREEN);
    screen.line(250, 108, 210, 620, 14, 0x0020_3850);
    screen.line(250, 108, 210, 620, 8, 0x0030_5878);
    screen.text(70, 250, "Kimi City", 2, TEXT);
    screen.text(70, 278, "grid 7 / 12", 1, MUTED);
    if let Some((px, py)) = maps.pin {
        screen.circle(px, py, 16, RED);
        screen.circle(px, py, 6, TEXT);
        screen.fill(px - 3, py + 12, 6, 10, RED);
    } else {
        screen.text_centered(width / 2, 350, "Tap the map to drop a pin", 2, TEXT);
    }
    screen.round_rect(20, 636, width - 40, 104, 26, SURFACE);
    if let Some((px, py)) = maps.pin {
        screen.text(40, 654, "Pinned location", 2, TEXT);
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
        screen.text(40, 688, slice_str(&line[..used]), 1, MUTED);
    } else {
        screen.text(40, 654, "Maps preview", 2, TEXT);
        screen.text(40, 684, "Local, offline and tiny", 1, MUTED);
    }
}

fn draw_photos(screen: &Screen, photos: &Photos, shots: u32) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
    if let Some(index) = photos.selected {
        draw_photo(screen, index, 24, 118, width - 48, 470);
        screen.text(24, 608, "Photo", 2, TEXT);
        let mut buf = [0u8; 20];
        let value = unsigned((index + 1) as u64, &mut buf);
        screen.text(88, 608, value, 2, MUTED);
        screen.text(24, 646, "Tap anywhere to close", 1, MUTED);
        return;
    }
    screen.text(24, 102, "GALLERY", 1, MUTED);
    for index in 0..6usize {
        let column = index as i64 % 2;
        let row = index as i64 / 2;
        let x = 24 + column * 224;
        let y = 120 + row * 156;
        draw_photo(screen, index, x, y, 208, 144);
        screen.text(x + 6, y + 148 - 18, "img", 1, TEXT);
    }
    let mut buf = [0u8; 20];
    let shots_text = unsigned(shots as u64, &mut buf);
    screen.text(24, 620, "Camera shots:", 1, MUTED);
    screen.text(156, 620, shots_text, 1, TEXT);
    screen.text(24, 648, "Tap a photo to open it", 1, MUTED);
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
                screen.ring(x + width / 2, y + height / 2, 18 + ring * 12, 2, RED);
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
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
    if let Some(index) = files.preview {
        screen.round_rect(20, 104, width - 40, 560, 26, SURFACE);
        let title = slice_str(&files.names[index][..files.name_len[index] as usize]);
        screen.text(40, 120, title, 2, TEXT);
        if files.binary {
            screen.text(40, 160, "Binary file", 1, MUTED);
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
                screen.text(x, y, slice_str(&[byte]), 1, TEXT);
                x += 8;
                column += 1;
            }
        }
        screen.text(40, 642, "Tap to close", 1, MUTED);
        return;
    }
    let tab = if files.disk { "DISK" } else { "RAMFS" };
    screen.round_rect(24, 100, 194, 42, 21, if files.disk { SURFACE } else { RED_DIM });
    screen.text_centered(121, 110, "RAMFS", 2, if files.disk { MUTED } else { TEXT });
    screen.round_rect(230, 100, 226, 42, 21, if files.disk { RED_DIM } else { SURFACE });
    screen.text_centered(343, 110, "DISK", 2, if files.disk { TEXT } else { MUTED });
    for index in 0..files.count.min(18) {
        let y = 170 + index as i64 * 30;
        screen.round_rect(20, y, width - 40, 28, 14, if index % 2 == 0 { SURFACE } else { BLACK });
        screen.round_rect(34, y + 7, 14, 14, 3, MUTED);
        let label = slice_str(&files.names[index][..files.name_len[index] as usize]);
        screen.text(60, y + 10, label, 1, TEXT);
        let mut buf = [0u8; 20];
        let size = unsigned(files.sizes[index], &mut buf);
        screen.text(width - 28 - Screen::text_width(size, 1), y + 10, size, 1, MUTED);
    }
    screen.text(24, 730, tab, 1, MUTED);
}

fn draw_weather(screen: &Screen, weather: &Weather, clock: [u64; 7]) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
    screen.round_rect(20, 104, width - 40, 240, 30, SURFACE);
    let temp = if weather.fahrenheit { 72 } else { 22 };
    let _ = temp;
    let mut buf = [0u8; 20];
    let text = signed(temp, &mut buf);
    screen.text(40, 132, text, 5, TEXT);
    screen.text(40 + Screen::text_width(text, 5) + 8, 140, if weather.fahrenheit { "F" } else { "C" }, 2, MUTED);
    screen.text(40, 220, "Clear", 2, TEXT);
    screen.text(40, 252, if weather.fahrenheit { "Feels like 75 F" } else { "Feels like 24 C" }, 1, MUTED);
    screen.circle(392, 248, 34, 0x00f0_c050);
    for (dx, dy) in [(0, -1), (1, 0), (0, 1), (-1, 0), (1, 1), (-1, 1), (1, -1), (-1, -1)] {
        screen.line(392 + dx * 42, 248 + dy * 42, 392 + dx * 56, 248 + dy * 56, 2, 0x00f0_c050);
    }
    button(screen, 304, 104, 152, 40, if weather.fahrenheit { "USE C" } else { "USE F" }, SURFACE_HI, MUTED);
    screen.text(24, 370, "5-DAY FORECAST", 1, MUTED);
    let days = ["MON", "TUE", "WED", "THU", "FRI"];
    let highs = [24i64, 26, 23, 21, 25];
    let lows = [15i64, 17, 14, 12, 16];
    for index in 0..5 {
        let y = 398 + index as i64 * 64;
        let active = index == weather.day;
        screen.round_rect(20, y, width - 40, 54, 18, if active { SURFACE_HI } else { SURFACE });
        screen.text(40, y + 18, days[index], 2, if active { TEXT } else { MUTED });
        let bar = (highs[index] - 10) * 12;
        screen.round_rect(160, y + 24, bar, 6, 3, RED);
        screen.round_rect(160, y + 24, (lows[index] - 10) * 12, 6, 3, OUTLINE);
        let mut hi = [0u8; 20];
        let mut lo = [0u8; 20];
        let high = signed(if weather.fahrenheit { highs[index] * 9 / 5 + 32 } else { highs[index] }, &mut hi);
        let low = signed(if weather.fahrenheit { lows[index] * 9 / 5 + 32 } else { lows[index] }, &mut lo);
        screen.text(width - 108, y + 18, high, 2, TEXT);
        screen.text(width - 48, y + 18, low, 2, MUTED);
    }
    let _ = clock;
    screen.text(24, 730, "Updated from the RTC", 1, MUTED);
}

fn draw_terminal(screen: &Screen, terminal: &Terminal) {
    let width = screen.width();
    screen.fill(0, 93, width, screen.height() - 93, BLACK);
    let start = terminal.count.saturating_sub(TERM_LINES);
    let mut y = 108i64;
    for index in start..terminal.count {
        let length = terminal.line_len[index] as usize;
        screen.text(20, y, slice_str(&terminal.lines[index][..length]), 1, TEXT);
        y += 13;
    }
    screen.fill(20, 684, width - 40, 1, OUTLINE);
    screen.text(20, 700, "$", 2, RED);
    let shown = terminal.input_len.min(52);
    screen.text(44, 700, slice_str(&terminal.input[..shown]), 2, TEXT);
    if terminal.input_len == 0 {
        screen.text(44, 700, "type a command", 2, 0x0050_505a);
    }
    screen.text(20, 744, "help  ls  cat  date  df  mem  ps  uptime  clear", 1, MUTED);
}
