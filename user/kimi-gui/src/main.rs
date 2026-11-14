#![no_std]
#![no_main]

//! KimiOS phone shell with a Nothing OS / Material 3 inspired look:
//! monochrome surfaces, dot-matrix clock, red accent, tonal cards, and a
//! Quick Settings shade. Runs entirely in user space.

mod draw;
mod font;

use core::panic::PanicInfo;

use draw::Screen;
use libkimi as k;

const BLACK: u32 = 0x0000_0000;
const WALL_TOP: u32 = 0x0000_0000;
const WALL_BOTTOM: u32 = 0x0009_0911;
const SURFACE: u32 = 0x0014_1419;
const SURFACE_HI: u32 = 0x001e_1e25;
const SURFACE_MAX: u32 = 0x0027_272f;
const OUTLINE: u32 = 0x0035_3540;
const TEXT: u32 = 0x00f2_f2f6;
const MUTED: u32 = 0x0093_93a0;
const RED: u32 = 0x00e0_352b;
const RED_DIM: u32 = 0x005c_1a16;
const GREEN: u32 = 0x0060_d18a;

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
    Phone,
    Messages,
    Camera,
    Music,
    Clock,
    Settings,
    Calculator,
    Notes,
    Maps,
    Photos,
    Files,
    Weather,
}

const GRID: [AppKind; 12] = [
    AppKind::Phone,
    AppKind::Messages,
    AppKind::Camera,
    AppKind::Music,
    AppKind::Clock,
    AppKind::Settings,
    AppKind::Calculator,
    AppKind::Notes,
    AppKind::Maps,
    AppKind::Photos,
    AppKind::Files,
    AppKind::Weather,
];

const DOCK: [AppKind; 4] = [
    AppKind::Phone,
    AppKind::Messages,
    AppKind::Camera,
    AppKind::Settings,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scene {
    Lock,
    Home,
    App(AppKind),
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
    playing: bool,
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
            playing: true,
        }
    }
}

struct Ui {
    scene: Scene,
    touch: (i64, i64),
    press: Option<(i64, i64)>,
    calc: Calculator,
    settings: Settings,
    shade: bool,
}

impl Ui {
    fn new(width: i64, height: i64) -> Self {
        Self {
            scene: Scene::Lock,
            touch: (width / 2, height / 2),
            press: None,
            calc: Calculator::new(),
            settings: Settings::new(),
            shade: false,
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let Some((width, height, stride, _)) = k::fb_info() else {
        k::print("gui: no framebuffer\n");
        k::exit(1);
    };
    let Some(base) = k::fb_map() else {
        k::print("gui: fb_map failed\n");
        k::exit(1);
    };

    let screen = Screen::new(base, width, height, stride);
    let mut ui = Ui::new(screen.width(), screen.height());
    let mut last_minute = u64::MAX;
    let mut last_second = u64::MAX;
    let mut dirty = true;

    loop {
        let clock = clock();
        let (minute, second) = (clock[5], clock[6]);

        if ui.scene == Scene::App(AppKind::Clock) {
            if second != last_second {
                last_second = second;
                dirty = true;
            }
        } else if minute != last_minute {
            last_minute = minute;
            dirty = true;
        }

        if dirty {
            render(&screen, &ui, clock);
            dirty = false;
        }

        match k::input_poll() {
            Some(event) => {
                if handle(&screen, &mut ui, event, clock) {
                    dirty = true;
                }
            }
            None => k::sleep_ms(16),
        }
    }
}

fn handle(screen: &Screen, ui: &mut Ui, event: k::InputEvent, clock: [u64; 7]) -> bool {
    let (width, height) = (screen.width(), screen.height());

    match (event.kind, event.code) {
        (k::EV_ABS, k::ABS_X) => {
            ui.touch.0 = scale(event.value, width);
            false
        }
        (k::EV_ABS, k::ABS_Y) => {
            ui.touch.1 = scale(event.value, height);
            false
        }
        (k::EV_KEY, 1) if event.value != 0 => {
            k::exit(0);
        }
        (k::EV_KEY, k::BTN_LEFT) | (k::EV_KEY, k::BTN_TOUCH) => {
            if event.value == 0 {
                let released = ui.press.take();
                render(screen, ui, clock);
                if let Some(start) = released {
                    let dx = ui.touch.0 - start.0;
                    let dy = ui.touch.1 - start.1;
                    if dx * dx + dy * dy < 24 * 24 {
                        tapped(screen, ui, ui.touch, clock);
                    } else if !ui.shade && ui.scene == Scene::Lock && start.1 - ui.touch.1 > 70 {
                        unlock(screen, ui, clock);
                    }
                }
                true
            } else {
                ui.press = Some(ui.touch);
                true
            }
        }
        _ => false,
    }
}

fn tapped(screen: &Screen, ui: &mut Ui, point: (i64, i64), clock: [u64; 7]) {
    if ui.shade {
        shade_tap(screen, ui, point, clock);
        return;
    }
    if point.1 < 30 {
        toggle_shade(screen, ui, clock, true);
        return;
    }

    match ui.scene {
        Scene::Lock => unlock(screen, ui, clock),
        Scene::Home => {
            if music_hit(screen, point) {
                ui.settings.playing = !ui.settings.playing;
                render(screen, ui, clock);
            } else if let Some(app) = home_hit(screen, point) {
                open_app(screen, ui, app, clock);
            }
        }
        Scene::App(app) => {
            if back_hit(screen, point) {
                close_app(screen, ui, clock);
                return;
            }
            match app {
                AppKind::Calculator => {
                    if let Some(index) = calculator_hit(point) {
                        ui.calc.press(CALCULATOR_LABELS[index]);
                        render(screen, ui, clock);
                    }
                }
                AppKind::Settings => {
                    if let Some(row) = settings_hit(screen, point) {
                        toggle_settings(&mut ui.settings, row);
                        render(screen, ui, clock);
                    }
                }
                _ => {}
            }
        }
    }
}

fn shade_tap(screen: &Screen, ui: &mut Ui, point: (i64, i64), clock: [u64; 7]) {
    let height = screen.height();
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
        // Tapping a notification opens Messages/System stubs; just dismiss
        // the shade so the launcher is visible.
        toggle_shade(screen, ui, clock, false);
        return;
    }
    let _ = (height, clock);
}

fn toggle_shade(screen: &Screen, ui: &mut Ui, clock: [u64; 7], open: bool) {
    let sheet = 610;
    if open == ui.shade {
        return;
    }
    if open {
        for step in 0..=8 {
            let y = -sheet + sheet * ease(step, 8) / 100;
            draw_scene(screen, ui, clock);
            draw_shade(screen, ui, clock, y);
            k::sleep_ms(14);
        }
        ui.shade = true;
    } else {
        for step in 0..=8 {
            let y = -sheet * ease(step, 8) / 100;
            draw_scene(screen, ui, clock);
            draw_shade(screen, ui, clock, y);
            k::sleep_ms(14);
        }
        ui.shade = false;
    }
    render(screen, ui, clock);
}

fn unlock(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    let height = screen.height();
    for step in 0..=14 {
        let offset = height * (100 - ease(step, 14)) / 100;
        draw_wallpaper(screen, 0, true);
        draw_lock(screen, clock, 0);
        draw_home(screen, clock, ui, None, offset);
        draw_status_bar(screen, clock, ui);
        k::sleep_ms(14);
    }
    ui.scene = Scene::Home;
    render(screen, ui, clock);
}

fn open_app(screen: &Screen, ui: &mut Ui, app: AppKind, clock: [u64; 7]) {
    let width = screen.width();
    for step in 0..=14 {
        let offset = width * (100 - ease(step, 14)) / 100;
        draw_wallpaper(screen, 0, true);
        draw_home(screen, clock, ui, None, 0);
        draw_app(screen, ui, app, clock, offset);
        draw_status_bar(screen, clock, ui);
        k::sleep_ms(14);
    }
    ui.scene = Scene::App(app);
    render(screen, ui, clock);
}

fn close_app(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    let width = screen.width();
    if let Scene::App(app) = ui.scene {
        for step in 0..=14 {
            let offset = width * ease(step, 14) / 100;
            draw_wallpaper(screen, 0, true);
            draw_home(screen, clock, ui, None, 0);
            draw_app(screen, ui, app, clock, offset);
            draw_status_bar(screen, clock, ui);
            k::sleep_ms(14);
        }
    }
    ui.scene = Scene::Home;
    render(screen, ui, clock);
}

fn render(screen: &Screen, ui: &Ui, clock: [u64; 7]) {
    draw_scene(screen, ui, clock);
    if ui.shade {
        draw_shade(screen, ui, clock, 0);
    } else {
        draw_status_bar(screen, clock, ui);
    }
    if let Some(point) = ui.press {
        screen.ring(point.0, point.1, 18, 3, 0x00a0_a8b8);
        screen.circle(point.0, point.1, 3, TEXT);
    }
}

fn draw_scene(screen: &Screen, ui: &Ui, clock: [u64; 7]) {
    match ui.scene {
        Scene::Lock => {
            draw_wallpaper(screen, 0, true);
            draw_lock(screen, clock, 0);
        }
        Scene::Home => {
            draw_wallpaper(screen, 0, ui.settings.dark);
            let pressed = ui.press.and_then(|point| home_hit(screen, point));
            draw_home(screen, clock, ui, pressed, 0);
        }
        Scene::App(app) => {
            draw_app(screen, ui, app, clock, 0);
        }
    }
}

fn draw_wallpaper(screen: &Screen, y_offset: i64, dark: bool) {
    let (top, bottom) = if dark {
        (WALL_TOP, WALL_BOTTOM)
    } else {
        (0x00ee_ee_f2, 0x00c9_c9d4)
    };
    let height = screen.height() - y_offset;
    for row in 0..height {
        let absolute = (row + y_offset).clamp(0, screen.height());
        let color = lerp_color(top, bottom, (absolute * 255 / screen.height().max(1)) as u32);
        screen.fill(0, y_offset + row, screen.width(), 1, color);
    }
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

fn draw_status_bar(screen: &Screen, clock: [u64; 7], ui: &Ui) {
    let width = screen.width();
    screen.fill(0, 0, width, 28, BLACK);

    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    screen.text(16, 7, str_of(&time), 2, TEXT);

    // Notification dot.
    screen.circle(98, 14, 3, RED);

    // Signal bars.
    for index in 0..4 {
        let bar = 4 + index * 4;
        screen.fill(
            width - 104 + index * 7,
            22 - bar,
            4,
            bar,
            if index < 3 { TEXT } else { MUTED },
        );
    }
    screen.text(width - 76, 7, "5G", 1, MUTED);

    // Battery.
    let battery_x = width - 44;
    screen.round_rect(battery_x, 8, 30, 15, 4, TEXT);
    screen.round_rect(battery_x + 2, 10, 26, 11, 3, BLACK);
    let charge = 20 * (if ui.settings.torch { 5 } else { 14 }) / 20;
    screen.round_rect(battery_x + 4, 12, charge, 7, 2, GREEN);
    screen.fill(battery_x + 31, 12, 2, 7, TEXT);
}

fn draw_lock(screen: &Screen, clock: [u64; 7], y_offset: i64) {
    let width = screen.width();
    let center = width / 2;

    let mut date = [0u8; 16];
    format_date(clock, &mut date);
    screen.text(28, 58 + y_offset, str_of(&date), 2, MUTED);
    screen.text(28, 82 + y_offset, "KIMIOS PREVIEW", 1, 0x0060_606c);

    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    let cells = 9;
    let clock_width = Screen::dots_text_width(str_of(&time), cells);
    screen.text_dots(
        center - clock_width / 2,
        120 + y_offset,
        str_of(&time),
        cells,
        3,
        TEXT,
    );

    notification_card(
        screen,
        28,
        260 + y_offset,
        RED,
        "Messages",
        "3 new messages",
        "now",
    );
    notification_card(
        screen,
        28,
        366 + y_offset,
        0x00b8_c4ff,
        "System",
        "KimiOS 0.1 is up to date",
        "2m",
    );

    // Fingerprint target: three broken ridges opening at the bottom.
    for (radius, inner, outer) in [(33i64, 6usize, 54usize), (25, 9, 51), (17, 12, 48)] {
        draw_arc(screen, center, 540 + y_offset, radius, inner, outer, 3, RED);
    }
    screen.text_centered(center, 590 + y_offset, "Touch to unlock", 2, TEXT);
    screen.text_centered(center, 620 + y_offset, "or swipe up", 1, 0x0060_606c);

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
    screen.round_rect(x, y, width, 92, 26, SURFACE);
    screen.circle(x + 38, y + 46, 16, accent);
    screen.circle(x + 38, y + 46, 6, BLACK);
    screen.text(x + 68, y + 20, title, 2, TEXT);
    screen.text(x + 68, y + 48, body, 1, MUTED);
    screen.text(x + width - 34, y + 22, age, 1, 0x0060_606c);
}

fn draw_home(
    screen: &Screen,
    clock: [u64; 7],
    ui: &Ui,
    pressed: Option<AppKind>,
    y_offset: i64,
) {
    let (width, height) = (screen.width(), screen.height());

    // At-a-glance / clock widget.
    screen.round_rect_blend(24, 44 + y_offset, width - 48, 82, 26, SURFACE, 235);
    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    screen.text(48, 56 + y_offset, str_of(&time), 3, TEXT);
    let mut date = [0u8; 16];
    format_date(clock, &mut date);
    screen.text(48, 94 + y_offset, str_of(&date), 1, MUTED);
    screen.text(width - 48 - Screen::text_width("22", 3), 60 + y_offset, "22", 3, RED);
    screen.text(width - 76, 96 + y_offset, "CLEAR", 1, MUTED);

    // Search pill.
    screen.round_rect(24, 142 + y_offset, width - 48, 54, 27, SURFACE_HI);
    screen.ring(54, 169 + y_offset, 8, 2, MUTED);
    screen.line(60, 175 + y_offset, 66, 181 + y_offset, 2, MUTED);
    screen.text(80, 162 + y_offset, "Search", 2, MUTED);

    // Music widget.
    let music_y = 212 + y_offset;
    screen.round_rect(24, music_y, width - 48, 100, 28, SURFACE);
    screen.round_rect(42, music_y + 18, 64, 64, 20, SURFACE_HI);
    screen.circle(74, music_y + 50, 18, RED_DIM);
    screen.circle(74, music_y + 50, 7, RED);
    screen.text(122, music_y + 20, "KimiOS Sessions", 2, TEXT);
    screen.text(122, music_y + 48, "Nothing OS / Ambient", 1, MUTED);
    draw_play(screen, width - 76, music_y + 50, ui.settings.playing);
    screen.round_rect(122, music_y + 76, width - 190, 4, 2, OUTLINE);
    screen.round_rect(122, music_y + 76, (width - 190) * 2 / 5, 4, 2, RED);

    // App grid.
    let pressed = pressed;
    for (index, app) in GRID.iter().enumerate() {
        let (cx, cy) = grid_center(width, index);
        draw_app_icon(screen, *app, cx, cy + y_offset, 60, pressed == Some(*app));
        screen.text_centered(cx, cy + 40 + y_offset, app.label(), 1, MUTED);
    }

    // Material 3 page indicator.
    let dots_y = height - 152 + y_offset;
    screen.round_rect(width / 2 - 28, dots_y - 3, 26, 6, 3, RED);
    for index in 1..3 {
        screen.circle(width / 2 - 2 + index as i64 * 18, dots_y, 3, OUTLINE);
    }

    // Dock.
    let dock_y = height - 120 + y_offset;
    screen.round_rect_blend(20, dock_y, width - 40, 92, 34, SURFACE, 225);
    for (index, app) in DOCK.iter().enumerate() {
        let cx = 76 + index as i64 * 109;
        draw_app_icon(screen, *app, cx, dock_y + 46, 56, pressed == Some(*app));
    }

    draw_home_indicator(screen);
}

fn draw_app_icon(screen: &Screen, app: AppKind, cx: i64, cy: i64, size: i64, pressed: bool) {
    let shrink = if pressed { 5 } else { 0 };
    let size = size - shrink;
    let tile = if pressed { SURFACE_MAX } else { SURFACE_HI };
    screen.round_rect(cx - size / 2, cy - size / 2, size, size, size / 4, tile);
    screen.round_rect(
        cx - size / 2 + 1,
        cy - size / 2 + 1,
        size - 2,
        size - 2,
        size / 4 - 1,
        tile,
    );
    let glyph = if app == AppKind::Music { RED } else { TEXT };
    draw_glyph(screen, app, cx, cy, size, glyph);
    if app == AppKind::Messages {
        screen.circle(cx + size / 2 - 5, cy - size / 2 + 5, 6, RED);
        screen.circle(cx + size / 2 - 5, cy - size / 2 + 5, 2, BLACK);
    }
}

fn draw_glyph(screen: &Screen, app: AppKind, cx: i64, cy: i64, size: i64, color: u32) {
    let s = size;
    let thin = (s / 16).max(2);
    match app {
        AppKind::Phone => {
            screen.line(cx - s / 7, cy - s / 7, cx + s / 9, cy + s / 8, s / 8, color);
            screen.circle(cx - s / 7, cy - s / 7, s / 10, color);
            screen.circle(cx + s / 8, cy + s / 8, s / 10, color);
        }
        AppKind::Messages => {
            screen.round_rect(cx - s / 4, cy - s / 5, s / 2, s / 3, s / 12, color);
            screen.line(cx - s / 12, cy + s / 8, cx - s / 5, cy + s / 4, s / 12, color);
            screen.line(cx - s / 12, cy + s / 8, cx + s / 20, cy + s / 10, s / 12, color);
        }
        AppKind::Camera => {
            screen.round_rect(cx - s / 3, cy - s / 6, s * 2 / 3, s / 2, s / 10, color);
            screen.round_rect(
                cx - s / 3 + 2,
                cy - s / 6 + 2,
                s * 2 / 3 - 4,
                s / 2 - 4,
                s / 12,
                SURFACE_HI,
            );
            screen.fill(cx - s / 6, cy - s / 3, s / 5, s / 12, color);
            screen.ring(cx, cy + s / 24, s / 5, 2, color);
            screen.circle(cx, cy + s / 24, s / 9, SURFACE_HI);
        }
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
                SURFACE_HI,
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
                SURFACE_HI,
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
            screen.circle(cx, cy, s / 10, SURFACE_HI);
        }
        AppKind::Files => {
            screen.round_rect(cx - s / 3, cy - s / 5, s * 2 / 3, s / 2, s / 14, color);
            screen.round_rect(
                cx - s / 3 + 2,
                cy - s / 5 + 2,
                s * 2 / 3 - 4,
                s / 2 - 4,
                s / 18,
                SURFACE_HI,
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
    }
}

fn draw_play(screen: &Screen, cx: i64, cy: i64, playing: bool) {
    screen.ring(cx, cy, 20, 2, OUTLINE);
    if playing {
        screen.fill(cx - 6, cy - 8, 4, 16, TEXT);
        screen.fill(cx + 2, cy - 8, 4, 16, TEXT);
    } else {
        screen.line(cx - 4, cy - 8, cx - 4, cy + 8, 2, TEXT);
        screen.line(cx - 4, cy - 8, cx + 8, cy, 2, TEXT);
        screen.line(cx - 4, cy + 8, cx + 8, cy, 2, TEXT);
        screen.line(cx - 2, cy, cx + 7, cy, 2, TEXT);
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
    screen.round_rect(width / 2 - 62, screen.height() - 20, 124, 5, 2, TEXT);
}

fn draw_app(screen: &Screen, ui: &Ui, app: AppKind, clock: [u64; 7], x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    let height = screen.height();
    screen.fill(x, 0, width - x, height, BLACK);

    // Large Material 3 top app bar.
    draw_back(screen, x + 40, 64);
    screen.text(x + 78, 48, app.label(), 2, TEXT);
    screen.fill(x, 92, width, 1, 0x0020_2026);

    match app {
        AppKind::Calculator => draw_calculator(screen, ui, x),
        AppKind::Clock => draw_clock_app(screen, clock, x),
        AppKind::Settings => draw_settings(screen, &ui.settings, x),
        _ => draw_placeholder(screen, app, x),
    }
    draw_home_indicator(screen);
}

fn draw_back(screen: &Screen, cx: i64, cy: i64) {
    screen.line(cx + 9, cy - 12, cx - 9, cy, 3, TEXT);
    screen.line(cx - 9, cy, cx + 9, cy + 12, 3, TEXT);
}

fn draw_calculator(screen: &Screen, ui: &Ui, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    screen.fill(x, 93, width, screen.height() - 93, BLACK);

    screen.round_rect(x + 20, 112, width - 40, 148, 30, SURFACE);
    let mut digits = [0u8; 20];
    let text = format_i64(ui.calc.current, &mut digits);
    let scale = if text.len() > 9 { 3 } else { 4 };
    let text_width = Screen::text_width(text, scale);
    screen.text(x + width - 44 - text_width, 176, text, scale, TEXT);
    if let Some(op) = ui.calc.pending {
        screen.text(x + width - 44 - 20, 136, str_of(&[op]), 2, RED);
    }

    for row in 0..5usize {
        for column in 0..4usize {
            let label = CALCULATOR_LABELS[row * 4 + column];
            if label.is_empty() {
                continue;
            }
            let (bx, by, bw, bh) = calculator_button(x, row, column);
            let (bw, color, text_color) = if label == "=" {
                (bw * 2 + 10, RED, TEXT)
            } else if label == "AC" {
                (bw, SURFACE_MAX, RED)
            } else {
                (bw, SURFACE_HI, TEXT)
            };
            let pressed = ui.press.map_or(false, |point| {
                point.0 >= bx && point.0 < bx + bw && point.1 >= by && point.1 < by + bh
            });
            let color = if pressed { SURFACE_MAX } else { color };
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

    screen.fill(x, 93, screen.width(), screen.height() - 93, BLACK);

    // Minute dots around the face, Nothing style.
    for index in 0..60 {
        let dx = CLOCK_X[index];
        let dy = CLOCK_Y[index];
        let big = index % 5 == 0;
        screen.circle(
            cx + dx * (radius - 10) / 1024,
            cy + dy * (radius - 10) / 1024,
            if big { 3 } else { 1 },
            if big { TEXT } else { OUTLINE },
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
        TEXT,
    );
    let minute_index = clock[5] as usize;
    screen.line(
        cx,
        cy,
        cx + CLOCK_X[minute_index] * 112 / 1024,
        cy + CLOCK_Y[minute_index] * 112 / 1024,
        5,
        TEXT,
    );
    let second_index = clock[6] as usize;
    screen.line(
        cx,
        cy,
        cx + CLOCK_X[second_index] * 124 / 1024,
        cy + CLOCK_Y[second_index] * 124 / 1024,
        2,
        RED,
    );
    screen.circle(cx, cy, 7, RED);

    let mut time = [0u8; 8];
    let length = format_hms(clock[4], clock[5], clock[6], &mut time);
    let cell = 6;
    let text = str_of(&time[..length]);
    let width = Screen::dots_text_width(text, cell);
    screen.text_dots(cx - width / 2, 570, text, cell, 2, TEXT);

    let mut date = [0u8; 16];
    format_date(clock, &mut date);
    screen.text_centered(cx, 650, str_of(&date), 2, MUTED);
}

fn draw_settings(screen: &Screen, settings: &Settings, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    screen.fill(x, 93, width, screen.height() - 93, BLACK);

    screen.text(x + 24, 112, "CONNECTIVITY", 1, MUTED);
    let rows = [
        ("Wi-Fi", "KimiOS-5G", settings.wifi, RED),
        ("Bluetooth", "Not connected", settings.bluetooth, 0x00b8_c4ff),
        ("Do Not Disturb", "Off", settings.dnd, 0x00d8_b060),
    ];
    for (index, (label, subtitle, value, accent)) in rows.iter().enumerate() {
        let y = 136 + index as i64 * 100;
        screen.round_rect(x + 20, y, width - 40, 88, 28, SURFACE);
        screen.circle(x + 58, y + 44, 18, *accent);
        screen.text(x + 88, y + 24, label, 2, TEXT);
        screen.text(x + 88, y + 52, subtitle, 1, MUTED);
        draw_switch(screen, x + width - 92, y + 28, *value);
    }

    screen.text(x + 24, 456, "APPEARANCE", 1, MUTED);
    screen.round_rect(x + 20, 480, width - 40, 88, 28, SURFACE);
    screen.circle(x + 58, 524, 18, 0x009c_74e8);
    screen.text(x + 88, 504, "Dark mode", 2, TEXT);
    screen.text(x + 88, 532, "Nothing palette", 1, MUTED);
    draw_switch(screen, x + width - 92, 508, settings.dark);

    screen.round_rect(x + 20, 596, width - 40, 120, 28, SURFACE);
    screen.text(x + 44, 618, "About KimiOS", 2, TEXT);
    screen.text(x + 44, 650, "ARM64 bare metal", 1, MUTED);
    screen.text(x + 44, 672, "480x800 - Material 3 / Nothing", 1, MUTED);
    screen.text(x + 44, 694, "Build 0.1", 1, 0x0060_606c);
}

fn draw_switch(screen: &Screen, x: i64, y: i64, on: bool) {
    let track = if on { RED_DIM } else { SURFACE_MAX };
    screen.round_rect(x, y, 52, 32, 16, track);
    if on {
        screen.round_rect(x, y, 52, 32, 16, RED_DIM);
    }
    let knob = if on { x + 38 } else { x + 14 };
    screen.circle(knob, y + 16, 11, if on { RED } else { MUTED });
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
    if point.0 >= 20 && point.1 >= 480 && point.1 < 568 {
        return Some(3);
    }
    None
}

fn toggle_settings(settings: &mut Settings, row: usize) {
    match row {
        0 => settings.wifi = !settings.wifi,
        1 => settings.bluetooth = !settings.bluetooth,
        2 => settings.dnd = !settings.dnd,
        _ => settings.dark = !settings.dark,
    }
}

fn draw_placeholder(screen: &Screen, app: AppKind, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    let cx = x + width / 2;
    screen.fill(x, 93, width, screen.height() - 93, BLACK);
    screen.round_rect(cx - 52, 250, 104, 104, 30, SURFACE);
    draw_glyph(screen, app, cx, 302, 100, TEXT);
    screen.text_centered(cx, 400, app.label(), 3, TEXT);
    screen.round_rect(cx - 110, 460, 220, 52, 26, SURFACE);
    screen.text_centered(cx, 476, "Coming soon", 2, MUTED);
    screen.text_centered(cx, 540, "Material 3 / Nothing concept", 1, 0x0060_606c);
}

fn draw_shade(screen: &Screen, ui: &Ui, clock: [u64; 7], y_offset: i64) {
    let width = screen.width();
    let height = screen.height();
    let sheet = 610;

    // Scrim over the underlying scene.
    screen.fill_blend(0, 0, width, height, BLACK, 160);

    // Sheet with rounded bottom corners.
    screen.round_rect(0, y_offset - 40, width, sheet + 40, 40, 0x000a_0a0f);
    screen.round_rect_blend(0, y_offset + 40, width, sheet - 40, 0, 0x000a_0a0f, 255);

    screen.round_rect(width / 2 - 20, y_offset + 12, 40, 4, 2, 0x0050_505a);

    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    screen.text(28, y_offset + 30, str_of(&time), 2, TEXT);
    let title = "Quick settings";
    let title_x = (width - Screen::text_width(title, 1)) / 2;
    screen.text(title_x, y_offset + 34, title, 1, MUTED);

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
        let card = if enabled { RED_DIM } else { SURFACE_HI };
        screen.round_rect(x, y, 208, 96, 30, card);
        shade_glyph(screen, index, x + 26, y + 30, enabled);
        screen.text(x + 22, y + 56, title, 1, TEXT);
        let subtitle_x = x + 208 - 14 - Screen::text_width(subtitle, 1);
        screen.text(subtitle_x, y + 56, subtitle, 1, MUTED);
    }

    // Brightness slider.
    screen.text(24, y_offset + 300, "BRIGHTNESS", 1, MUTED);
    screen.round_rect(24, y_offset + 322, width - 48, 22, 11, SURFACE_HI);
    let fill = (width - 48) * ui.settings.brightness / 100;
    screen.round_rect(24, y_offset + 322, fill, 22, 11, TEXT);
    screen.circle(24 + fill, y_offset + 333, 14, RED);

    screen.text(24, y_offset + 366, "NOTIFICATIONS", 1, MUTED);
    notification_card(
        screen,
        24,
        y_offset + 390,
        RED,
        "Messages",
        "3 new messages",
        "now",
    );
    notification_card(
        screen,
        24,
        y_offset + 490,
        0x00b8_c4ff,
        "System",
        "KimiOS 0.1 is up to date",
        "2m",
    );

    // Settings shortcut in the header.
    screen.ring(width - 42, y_offset + 40, 11, 2, TEXT);
    screen.circle(width - 42, y_offset + 40, 3, TEXT);
}

fn shade_glyph(screen: &Screen, index: usize, x: i64, y: i64, enabled: bool) {
    let color = if enabled { TEXT } else { MUTED };
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

fn back_hit(screen: &Screen, point: (i64, i64)) -> bool {
    let bottom = screen.height() - 30;
    (point.0 < 76 && point.1 > 30 && point.1 < 96) || point.1 > bottom
}

fn music_hit(screen: &Screen, point: (i64, i64)) -> bool {
    point.0 >= screen.width() - 108
        && point.0 <= screen.width() - 44
        && point.1 >= 224
        && point.1 <= 300
}

fn grid_center(width: i64, index: usize) -> (i64, i64) {
    let column = index % 4;
    let row = index / 4;
    let cx = width / 2 + (column as i64 - 1) * 112 - 56;
    let cy = 356 + row as i64 * 114;
    (cx, cy)
}

fn home_hit(screen: &Screen, point: (i64, i64)) -> Option<AppKind> {
    let width = screen.width();
    for (index, app) in GRID.iter().enumerate() {
        let (cx, cy) = grid_center(width, index);
        if (point.0 - cx).abs() <= 38 && (point.1 - cy).abs() <= 38 {
            return Some(*app);
        }
    }
    let dock_y = screen.height() - 120 + 46;
    for (index, app) in DOCK.iter().enumerate() {
        let cx = 76 + index as i64 * 109;
        if (point.0 - cx).abs() <= 34 && (point.1 - dock_y).abs() <= 34 {
            return Some(*app);
        }
    }
    None
}

impl AppKind {
    fn label(self) -> &'static str {
        match self {
            AppKind::Phone => "Phone",
            AppKind::Messages => "Messages",
            AppKind::Camera => "Camera",
            AppKind::Music => "Music",
            AppKind::Clock => "Clock",
            AppKind::Settings => "Settings",
            AppKind::Calculator => "Calculator",
            AppKind::Notes => "Notes",
            AppKind::Maps => "Maps",
            AppKind::Photos => "Photos",
            AppKind::Files => "Files",
            AppKind::Weather => "Weather",
        }
    }
}

fn clock() -> [u64; 7] {
    k::rtc().unwrap_or([0, 2026, 1, 1, 0, 0, 0])
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

fn lerp_color(top: u32, bottom: u32, t: u32) -> u32 {
    let mix = |shift: u32| {
        let a = (top >> shift) & 0xff;
        let b = (bottom >> shift) & 0xff;
        ((a * (255 - t) + b * t) / 255) & 0xff
    };
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::exit(2)
}