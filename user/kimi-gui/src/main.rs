#![no_std]
#![no_main]

//! KimiOS phone shell: lock screen, home grid, dock, status bar and a few
//! working apps (Calculator, Clock, Settings). Drawn entirely in user space
//! into the mapped framebuffer; input comes from the shared event queue.

mod draw;
mod font;

use core::panic::PanicInfo;

use draw::Screen;
use libkimi as k;

const WALL_TOP: u32 = 0x0018_2d52;
const WALL_BOTTOM: u32 = 0x0007_0d18;
const STATUS_BG: u32 = 0x000b_1524;
const WHITE: u32 = 0x00f3_f6fa;
const SUBTEXT: u32 = 0x0094_a4b8;
const PANEL: u32 = 0x0016_202e;
const PANEL_HI: u32 = 0x0022_2e40;
const ACCENT: u32 = 0x003d_8bff;
const RED: u32 = 0x00ff_5a5a;
const GREEN: u32 = 0x0035_c46a;

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
}

const GRID: [AppKind; 8] = [
    AppKind::Phone,
    AppKind::Messages,
    AppKind::Camera,
    AppKind::Music,
    AppKind::Clock,
    AppKind::Settings,
    AppKind::Calculator,
    AppKind::Notes,
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
        // Pressing an operator twice (or `=` right after one) just replaces
        // or clears the pending operation; it must not re-apply it.
        if self.fresh {
            if self.pending.is_some() {
                self.pending = if op == b'=' { None } else { Some(op) };
                return;
            }
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
}

impl Settings {
    const fn new() -> Self {
        Self {
            wifi: true,
            bluetooth: false,
            dark: true,
        }
    }
}

struct Ui {
    scene: Scene,
    touch: (i64, i64),
    press: Option<(i64, i64)>,
    calc: Calculator,
    settings: Settings,
}

impl Ui {
    fn new(width: i64, height: i64) -> Self {
        Self {
            scene: Scene::Lock,
            touch: (width / 2, height / 2),
            press: None,
            calc: Calculator::new(),
            settings: Settings::new(),
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

/// Returns true when the screen needs a redraw.
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
                        tap(screen, ui, ui.touch, clock);
                    } else if ui.scene == Scene::Lock && start.1 - ui.touch.1 > 70 {
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

fn tap(screen: &Screen, ui: &mut Ui, point: (i64, i64), clock: [u64; 7]) {
    match ui.scene {
        Scene::Lock => unlock(screen, ui, clock),
        Scene::Home => {
            if let Some(app) = home_hit(screen, point) {
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
                    if let Some(index) = calculator_hit(screen, point) {
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

fn unlock(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    let height = screen.height();
    for step in 0..=16 {
        let offset = height - height * step / 16;
        draw_wallpaper(screen, 0, ui.settings.dark);
        draw_lock(screen, clock, ui.touch, 0);
        draw_home(screen, clock, ui.touch, None, offset, ui.settings.dark);
        draw_status_bar(screen, clock);
        k::sleep_ms(14);
    }
    ui.scene = Scene::Home;
    render(screen, ui, clock);
}

fn open_app(screen: &Screen, ui: &mut Ui, app: AppKind, clock: [u64; 7]) {
    let width = screen.width();
    for step in 0..=14 {
        let offset = width - width * step / 14;
        draw_wallpaper(screen, 0, ui.settings.dark);
        draw_home(screen, clock, ui.touch, None, 0, ui.settings.dark);
        draw_app(screen, ui, app, clock, offset);
        draw_status_bar(screen, clock);
        k::sleep_ms(14);
    }
    ui.scene = Scene::App(app);
    render(screen, ui, clock);
}

fn close_app(screen: &Screen, ui: &mut Ui, clock: [u64; 7]) {
    let width = screen.width();
    if let Scene::App(app) = ui.scene {
        for step in 0..=14 {
            let offset = width * step / 14;
            draw_wallpaper(screen, 0, ui.settings.dark);
            draw_home(screen, clock, ui.touch, None, 0, ui.settings.dark);
            draw_app(screen, ui, app, clock, offset);
            draw_status_bar(screen, clock);
            k::sleep_ms(14);
        }
    }
    ui.scene = Scene::Home;
    render(screen, ui, clock);
}

fn render(screen: &Screen, ui: &Ui, clock: [u64; 7]) {
    draw_wallpaper(screen, 0, ui.settings.dark);
    match ui.scene {
        Scene::Lock => {
            draw_lock(screen, clock, ui.touch, 0);
            draw_status_bar(screen, clock);
        }
        Scene::Home => {
            draw_home(screen, clock, ui.touch, None, 0, ui.settings.dark);
            draw_status_bar(screen, clock);
        }
        Scene::App(app) => {
            draw_app(screen, ui, app, clock, 0);
            draw_status_bar(screen, clock);
        }
    }
    if let Some(point) = ui.press {
        screen.ring(point.0, point.1, 18, 3, 0x00c8_d8f0);
        screen.circle(point.0, point.1, 3, 0x00ff_ffff);
    }
}

fn draw_wallpaper(screen: &Screen, y_offset: i64, dark: bool) {
    let (top, bottom) = if dark {
        (WALL_TOP, WALL_BOTTOM)
    } else {
        (0x005c_7fb0, 0x001d_3350)
    };
    let height = screen.height() - y_offset;
    for row in 0..height {
        let absolute = (row + y_offset).clamp(0, screen.height());
        let color = lerp_color(top, bottom, (absolute * 255 / screen.height().max(1)) as u32);
        screen.fill(0, y_offset + row, screen.width(), 1, color);
    }
    // Soft decorative circles.
    let halo = if dark { 0x0020_3a63 } else { 0x0072_94c0 };
    screen.circle(screen.width() - 70, 120 + y_offset, 150, halo);
    screen.circle(40, 640 + y_offset, 170, halo);
    screen.circle(screen.width() - 60, 700 + y_offset, 110, halo);
}

fn draw_status_bar(screen: &Screen, clock: [u64; 7]) {
    let width = screen.width();
    screen.fill(0, 0, width, 30, STATUS_BG);

    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    screen.text(16, 8, str_of(&time), 2, WHITE);

    screen.text_centered(width / 2, 11, "KimiOS", 1, SUBTEXT);

    // Signal bars.
    for index in 0..4 {
        let height = 5 + index * 4;
        screen.fill(
            width - 106 + index * 8,
            24 - height,
            5,
            height,
            if index < 3 { WHITE } else { SUBTEXT },
        );
    }

    // Battery.
    let battery_x = width - 48;
    screen.round_rect(battery_x, 7, 34, 17, 4, WHITE);
    screen.round_rect(battery_x + 2, 9, 30, 13, 3, STATUS_BG);
    screen.round_rect(battery_x + 4, 11, 21, 9, 2, GREEN);
    screen.fill(battery_x + 35, 12, 3, 7, WHITE);
}

fn draw_lock(screen: &Screen, clock: [u64; 7], touch: (i64, i64), y_offset: i64) {
    let width = screen.width();
    let center = width / 2;

    let mut time = [0u8; 5];
    format_time(clock[4], clock[5], &mut time);
    screen.text_centered(center, 180 + y_offset, str_of(&time), 9, WHITE);

    let mut date = [0u8; 16];
    format_date(clock, &mut date);
    screen.text_centered(center, 280 + y_offset, str_of(&date), 2, SUBTEXT);

    // Padlock.
    let lock_y = 380 + y_offset;
    screen.ring(center, lock_y, 26, 7, SUBTEXT);
    screen.fill(center - 26, lock_y, 52, 22, WALL_TOP);
    screen.round_rect(center - 34, lock_y + 14, 68, 54, 12, SUBTEXT);
    screen.circle(center, lock_y + 36, 6, WALL_TOP);
    screen.fill(center - 2, lock_y + 38, 4, 12, WALL_TOP);

    screen.text_centered(center, 600 + y_offset, "Swipe up to unlock", 2, SUBTEXT);
    screen.text_centered(center, 640 + y_offset, "or tap anywhere", 1, 0x0070_8298);

    if y_offset == 0 {
        let _ = touch;
    }
    draw_home_indicator(screen);
}

fn draw_home(
    screen: &Screen,
    clock: [u64; 7],
    touch: (i64, i64),
    pressed: Option<AppKind>,
    y_offset: i64,
    dark: bool,
) {
    let (width, height) = (screen.width(), screen.height());
    let _ = (clock, touch);

    draw_wallpaper(screen, y_offset, dark);
    for (index, app) in GRID.iter().enumerate() {
        let (cx, cy) = grid_center(width, index);
        draw_app_icon(screen, *app, cx, cy + y_offset, 64, pressed == Some(*app));
        screen.text_centered(cx, cy + 46 + y_offset, app.label(), 1, WHITE);
    }

    // Dock.
    let dock_y = height - 118 + y_offset;
    screen.round_rect(24, dock_y, width - 48, 90, 26, 0x0018_2436);
    for (index, app) in DOCK.iter().enumerate() {
        let cx = 78 + index as i64 * 108;
        draw_app_icon(screen, *app, cx, dock_y + 45, 56, pressed == Some(*app));
    }

    // Page dots.
    let dots_y = height - 150 + y_offset;
    for index in 0..3 {
        let color = if index == 0 { WHITE } else { 0x0050_6a88 };
        screen.circle(width / 2 - 24 + index as i64 * 24, dots_y, 4, color);
    }

    draw_home_indicator(screen);
}

fn draw_app_icon(screen: &Screen, app: AppKind, cx: i64, cy: i64, size: i64, pressed: bool) {
    let size = if pressed { size - 6 } else { size };
    let accent = if pressed { darken(app.accent(), 35) } else { app.accent() };
    screen.round_rect(cx - size / 2, cy - size / 2, size, size, size / 5, accent);
    draw_glyph(screen, app, cx, cy, size, WHITE);
    if app == AppKind::Messages {
        screen.circle(cx + size / 2 - 6, cy - size / 2 + 6, 7, RED);
        screen.circle(cx + size / 2 - 6, cy - size / 2 + 6, 3, WHITE);
    }
}

fn draw_glyph(screen: &Screen, app: AppKind, cx: i64, cy: i64, size: i64, color: u32) {
    let s = size;
    match app {
        AppKind::Phone => {
            screen.line(cx - s / 7, cy - s / 7, cx + s / 8, cy + s / 9, s / 7, color);
            screen.circle(cx - s / 6, cy - s / 6, s / 8, color);
            screen.circle(cx + s / 7, cy + s / 7, s / 8, color);
        }
        AppKind::Messages => {
            screen.round_rect(cx - s / 4, cy - s / 5, s / 2, s / 3, s / 12, color);
            screen.line(cx - s / 12, cy + s / 8, cx - s / 5, cy + s / 4, s / 12, color);
            screen.line(cx - s / 12, cy + s / 8, cx + s / 20, cy + s / 10, s / 12, color);
        }
        AppKind::Camera => {
            screen.round_rect(cx - s / 3, cy - s / 5, s * 2 / 3, s / 2, s / 12, color);
            screen.round_rect(
                cx - s / 3 + 3,
                cy - s / 5 + 3,
                s * 2 / 3 - 6,
                s / 2 - 6,
                s / 14,
                app.accent(),
            );
            screen.fill(cx - s / 6, cy - s / 3, s / 5, s / 12, color);
            screen.ring(cx, cy, s / 6, s / 12, color);
            screen.circle(cx, cy, s / 14, color);
        }
        AppKind::Music => {
            screen.circle(cx - s / 10, cy + s / 6, s / 9, color);
            screen.line(cx - s / 20, cy + s / 8, cx - s / 20, cy - s / 5, s / 14, color);
            screen.line(cx - s / 20, cy - s / 5, cx + s / 5, cy - s / 4, s / 12, color);
        }
        AppKind::Clock => {
            screen.ring(cx, cy, s / 3, s / 14, color);
            screen.line(cx, cy, cx, cy - s / 5, s / 16, color);
            screen.line(cx, cy, cx + s / 6, cy, s / 16, color);
            screen.circle(cx, cy, s / 22, color);
        }
        AppKind::Settings => {
            // Teeth around a thick ring, with an accent-coloured hole.
            for index in 0..8 {
                let dx = CLOCK_X[index * 7 % 60] * s / 2500;
                let dy = CLOCK_Y[index * 7 % 60] * s / 2500;
                let size = s / 8;
                screen.round_rect(
                    cx + dx - size / 2,
                    cy + dy - size / 2,
                    size,
                    size,
                    size / 3,
                    color,
                );
            }
            screen.ring(cx, cy, s / 3, s / 9, color);
            screen.circle(cx, cy, s / 9, app.accent());
        }
        AppKind::Calculator => {
            screen.round_rect(cx - s / 4, cy - s / 3, s / 2, s * 2 / 3, s / 12, color);
            screen.round_rect(cx - s / 4 + 3, cy - s / 3 + 3, s / 2 - 6, s * 2 / 3 - 6, s / 16, app.accent());
            for row in 0..3 {
                for column in 0..3 {
                    screen.fill(
                        cx - s / 10 + column * s / 7,
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
            screen.round_rect(cx - s / 3 + 3, cy - s / 3 + 3, s * 2 / 3 - 6, s * 2 / 3 - 6, s / 16, app.accent());
            for row in 0..3 {
                screen.fill(cx - s / 5, cy - s / 6 + row * s / 6, s * 2 / 5, s / 20, color);
            }
        }
    }
}

fn draw_home_indicator(screen: &Screen) {
    let width = screen.width();
    screen.round_rect(width / 2 - 70, screen.height() - 22, 140, 6, 3, 0x00e0_e8f2);
}

fn draw_app(screen: &Screen, ui: &Ui, app: AppKind, clock: [u64; 7], x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    let height = screen.height();
    screen.fill(x, 0, width - x, height, 0x0008_0d16);

    // Header.
    screen.fill(x, 0, width, 96, 0x000c_1420);
    screen.fill(x, 95, width, 1, 0x0020_2c3e);
    draw_back(screen, x + 36, 56);
    screen.text_centered(x + width / 2, 48, app.label(), 2, WHITE);

    match app {
        AppKind::Calculator => draw_calculator(screen, ui, x),
        AppKind::Clock => draw_clock_app(screen, clock, x),
        AppKind::Settings => draw_settings(screen, &ui.settings, x),
        _ => draw_placeholder(screen, app, x),
    }
    draw_home_indicator(screen);
}

fn draw_back(screen: &Screen, cx: i64, cy: i64) {
    screen.line(cx + 12, cy - 14, cx - 12, cy, 4, WHITE);
    screen.line(cx - 12, cy, cx + 12, cy + 14, 4, WHITE);
}

fn draw_calculator(screen: &Screen, ui: &Ui, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    screen.fill(x, 96, width, screen.height() - 96, 0x0009_1018);

    screen.round_rect(x + 20, 112, width - 40, 132, 22, PANEL);
    let mut digits = [0u8; 20];
    let text = format_i64(ui.calc.current, &mut digits);
    let scale = if text.len() > 9 { 3 } else { 4 };
    let text_width = Screen::text_width(text, scale);
    screen.text(x + width - 36 - text_width, 168, text, scale, WHITE);

    for row in 0..5usize {
        for column in 0..4usize {
            let index = row * 4 + column;
            let label = CALCULATOR_LABELS[index];
            if label.is_empty() {
                continue;
            }
            let (bx, by, bw, bh) = calculator_button(x, row, column);
            let (bw, color) = if label == "=" {
                (bw * 2 + 10, ACCENT)
            } else {
                (bw, calculator_color(label))
            };
            let pressed = ui.press.map_or(false, |point| {
                point.0 >= bx && point.0 < bx + bw && point.1 >= by && point.1 < by + bh
            });
            let color = if pressed { darken(color, 40) } else { color };
            screen.round_rect(bx, by, bw, bh, 18, color);
            screen.text_centered(bx + bw / 2, by + bh / 2 - 12, label, 3, WHITE);
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
    let button_height = 88;
    let bx = x_offset + margin + column as i64 * (button_width + gap);
    let by = 268 + row as i64 * (button_height + 10);
    (bx, by, button_width, button_height)
}

fn calculator_color(label: &str) -> u32 {
    match label {
        "AC" => 0x00a8_3440,
        "/" | "*" | "-" | "+" | "=" => 0x002a_4a7a,
        "<" | "+/-" => 0x0020_2a3a,
        _ => PANEL_HI,
    }
}

fn calculator_hit(_screen: &Screen, point: (i64, i64)) -> Option<usize> {
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
    let cy = 400;
    let radius = 158;

    screen.fill(x, 96, screen.width(), screen.height() - 96, 0x0009_1018);
    screen.ring(cx, cy, radius, 6, WHITE);
    for index in 0..12 {
        let dx = CLOCK_X[(index * 5) as usize];
        let dy = CLOCK_Y[(index * 5) as usize];
        let inner = radius - 22;
        let outer = radius - 8;
        screen.line(
            cx + dx * inner / 1024,
            cy + dy * inner / 1024,
            cx + dx * outer / 1024,
            cy + dy * outer / 1024,
            4,
            if index % 3 == 0 { WHITE } else { SUBTEXT },
        );
    }

    let hour = clock[4] % 12;
    let hour_index = (hour * 5 + clock[5] / 12) as usize;
    screen.line(
        cx,
        cy,
        cx + CLOCK_X[hour_index] * 76 / 1024,
        cy + CLOCK_Y[hour_index] * 76 / 1024,
        10,
        WHITE,
    );
    let minute_index = clock[5] as usize;
    screen.line(
        cx,
        cy,
        cx + CLOCK_X[minute_index] * 120 / 1024,
        cy + CLOCK_Y[minute_index] * 120 / 1024,
        6,
        WHITE,
    );
    let second_index = clock[6] as usize;
    screen.line(
        cx,
        cy,
        cx + CLOCK_X[second_index] * 128 / 1024,
        cy + CLOCK_Y[second_index] * 128 / 1024,
        3,
        RED,
    );
    screen.circle(cx, cy, 9, ACCENT);

    let mut time = [0u8; 8];
    let length = format_hms(clock[4], clock[5], clock[6], &mut time);
    screen.text_centered(cx, 600, str_of(&time[..length]), 4, WHITE);

    let mut date = [0u8; 16];
    format_date(clock, &mut date);
    screen.text_centered(cx, 650, str_of(&date), 2, SUBTEXT);
}

fn draw_settings(screen: &Screen, settings: &Settings, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    screen.fill(x, 96, width, screen.height() - 96, 0x0009_1018);

    let rows = [
        ("Wi-Fi", settings.wifi, 0x003d_8bff),
        ("Bluetooth", settings.bluetooth, 0x003d_8bff),
        ("Dark mode", settings.dark, 0x009c_6bff),
    ];
    for (index, (label, value, accent)) in rows.iter().enumerate() {
        let y = 122 + index as i64 * 100;
        screen.round_rect(x + 20, y, width - 40, 84, 20, PANEL);
        screen.circle(x + 56, y + 42, 14, *accent);
        screen.text(x + 82, y + 30, label, 2, WHITE);
        draw_switch(screen, x + width - 96, y + 26, *value);
    }

    screen.round_rect(x + 20, 440, width - 40, 190, 22, PANEL);
    screen.text(x + 44, 468, "About KimiOS", 2, WHITE);
    screen.text(x + 44, 508, "ARM64 bare-metal shell", 1, SUBTEXT);
    screen.text(x + 44, 530, "480x800 phone interface", 1, SUBTEXT);
    screen.text(x + 44, 552, "Calculator, Clock, Settings", 1, SUBTEXT);
    screen.text(x + 44, 588, "Build 0.1", 1, 0x0068_7c94);
}

fn draw_switch(screen: &Screen, x: i64, y: i64, on: bool) {
    let track = if on { ACCENT } else { 0x0038_4252 };
    screen.round_rect(x, y, 56, 32, 16, track);
    let knob = if on { x + 40 } else { x + 16 };
    screen.circle(knob, y + 16, 12, WHITE);
}

fn settings_hit(screen: &Screen, point: (i64, i64)) -> Option<usize> {
    for index in 0..3 {
        let y = 122 + index as i64 * 100;
        if point.0 >= 20
            && point.0 < screen.width() - 20
            && point.1 >= y
            && point.1 < y + 84
        {
            return Some(index);
        }
    }
    None
}

fn toggle_settings(settings: &mut Settings, row: usize) {
    match row {
        0 => settings.wifi = !settings.wifi,
        1 => settings.bluetooth = !settings.bluetooth,
        _ => settings.dark = !settings.dark,
    }
}

fn draw_placeholder(screen: &Screen, app: AppKind, x_offset: i64) {
    let x = x_offset;
    let width = screen.width();
    let cx = x + width / 2;
    screen.fill(x, 96, width, screen.height() - 96, 0x0009_1018);
    draw_app_icon(screen, app, cx, 330, 96, false);
    screen.text_centered(cx, 430, app.label(), 3, WHITE);
    screen.text_centered(cx, 500, "Coming soon", 2, SUBTEXT);
    screen.text_centered(cx, 540, "This app is a stub in KimiOS 0.1", 1, 0x0068_7c94);
}

fn back_hit(screen: &Screen, point: (i64, i64)) -> bool {
    let bottom = screen.height() - 30;
    (point.0 < 70 && point.1 > 30 && point.1 < 96) || point.1 > bottom
}

fn grid_center(width: i64, index: usize) -> (i64, i64) {
    let column = index % 4;
    let row = index / 4;
    let cx = width / 2 + (column as i64 - 1) * 112 - 56;
    let cy = 190 + row as i64 * 140;
    (cx, cy)
}

fn home_hit(screen: &Screen, point: (i64, i64)) -> Option<AppKind> {
    let width = screen.width();
    for (index, app) in GRID.iter().enumerate() {
        let (cx, cy) = grid_center(width, index);
        if (point.0 - cx).abs() <= 40 && (point.1 - cy).abs() <= 40 {
            return Some(*app);
        }
    }
    let dock_y = screen.height() - 118 + 45;
    for (index, app) in DOCK.iter().enumerate() {
        let cx = 78 + index as i64 * 108;
        if (point.0 - cx).abs() <= 36 && (point.1 - dock_y).abs() <= 36 {
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
        }
    }

    fn accent(self) -> u32 {
        match self {
            AppKind::Phone => 0x0030_c060,
            AppKind::Messages => 0x003d_8bff,
            AppKind::Camera => 0x0070_7480,
            AppKind::Music => 0x00e0_4d6a,
            AppKind::Clock => 0x002c_3346,
            AppKind::Settings => 0x004a_5566,
            AppKind::Calculator => 0x00d8_8a2a,
            AppKind::Notes => 0x00e0_b040,
        }
    }
}

fn clock() -> [u64; 7] {
    k::rtc().unwrap_or([0, 2026, 1, 1, 0, 0, 0])
}

fn scale(value: u32, dimension: i64) -> i64 {
    (value as i64 * (dimension - 1)) / 32767
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
    let name = WEEKDAYS[weekday];
    let month_name = MONTHS[(month - 1) as usize];
    let mut length = 0;
    for byte in name.bytes() {
        buffer[length] = byte;
        length += 1;
    }
    buffer[length] = b',';
    buffer[length + 1] = b' ';
    length += 2;
    for byte in month_name.bytes() {
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

fn darken(color: u32, amount: u32) -> u32 {
    let red = ((color >> 16) & 0xff).saturating_sub(amount);
    let green = ((color >> 8) & 0xff).saturating_sub(amount);
    let blue = (color & 0xff).saturating_sub(amount);
    (red << 16) | (green << 8) | blue
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