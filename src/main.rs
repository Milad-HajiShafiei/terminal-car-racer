// src/main.rs
//! RAT RACER — a terminal highway chase built with ratatui.
//!
//!   cargo run --release
//!
//! Controls:
//!   ← → / A D .... steer        ↑ / W ......... gas
//!   ↓ / S ........ brake        SPACE ........ nitro (hold)
//!   P ............ pause        M ............ sound on/off
//!   1 2 3 ........ intensity    ENTER ........ start / retry
//!   Q / ESC ...... quit
//!
//! Best played in a truecolor terminal (kitty, wezterm, alacritty, iTerm2…).

use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use rand::Rng;
use rand::rngs::ThreadRng;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use ratatui::{Frame, Terminal};

// ── tuning ────────────────────────────────────────────────────────────────
const K_ROWS: f64 = 0.11; // km/h → screen rows per second
const SIGNS: [&str; 5] = [
    "CHEZ REMY",
    "GO GO GO!",
    "BON APPÉTIT",
    "NITRO TIME",
    "RATATOUILLE!",
];
const QUIPS: [&str; 4] = [
    "the rat walked away. the car didn't.",
    "insurance rates: now astronomical.",
    "rémy demands a recount.",
    "that other car came out of nowhere. probably.",
];
const SEDAN: [&str; 5] = [" ▄▄▄ ", "▐█▓█▌", "▐███▌", "▐█▓█▌", " ▀▀▀ "];
const TRUCK: [&str; 7] = [
    " ▄▄▄ ",
    "▐█▓█▌",
    "▐███▌",
    "▐█░█▌",
    "▐█░█▌",
    "▐███▌",
    " ▀▀▀ ",
];
const TRAFFIC_COLORS: [Color; 8] = [
    Color::Rgb(95, 165, 255),
    Color::Rgb(125, 220, 130),
    Color::Rgb(225, 228, 235),
    Color::Rgb(255, 200, 90),
    Color::Rgb(205, 130, 255),
    Color::Rgb(255, 145, 145),
    Color::Rgb(140, 220, 230),
    Color::Rgb(255, 170, 95),
];

// ── difficulty ────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
enum Difficulty {
    Chill,
    Rush,
    Mayhem,
}

impl Difficulty {
    const ALL: [Difficulty; 3] = [Difficulty::Chill, Difficulty::Rush, Difficulty::Mayhem];
    fn index(self) -> usize {
        self as usize
    }
    fn name(self) -> &'static str {
        match self {
            Difficulty::Chill => "CHILL",
            Difficulty::Rush => "RUSH",
            Difficulty::Mayhem => "MAYHEM",
        }
    }
    fn max_speed(self) -> f64 {
        match self {
            Difficulty::Chill => 160.0,
            Difficulty::Rush => 215.0,
            Difficulty::Mayhem => 265.0,
        }
    }
    fn spawn_gap(self) -> (f64, f64) {
        match self {
            Difficulty::Chill => (1.5, 2.3),
            Difficulty::Rush => (1.0, 1.6),
            Difficulty::Mayhem => (0.62, 1.05),
        }
    }
    fn traffic(self) -> (f64, f64) {
        match self {
            Difficulty::Chill => (50.0, 85.0),
            Difficulty::Rush => (58.0, 105.0),
            Difficulty::Mayhem => (66.0, 128.0),
        }
    }
    fn blurb(self) -> &'static str {
        match self {
            Difficulty::Chill => "sunday cruise · light traffic · top 160 km/h",
            Difficulty::Rush => "rush hour · dense lanes · top 215 km/h",
            Difficulty::Mayhem => "gridlock chaos · no mercy · top 265 km/h",
        }
    }
}

// ── state & entities ──────────────────────────────────────────────────────
#[derive(PartialEq, Clone, Copy)]
enum State {
    Menu,
    Countdown,
    Running,
    Paused,
    Crashing,
    GameOver,
}

struct Car {
    lane: usize,
    x: f64,
    y: f64,
    speed: f64,
    color: Color,
    truck: bool,
    passed: bool,
    wob: f64,
    wob_f: f64,
}

#[derive(Clone, Copy)]
struct Pickup {
    x: f64,
    y: f64,
    t: f64,
}
struct Particle {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    life: f64,
    max: f64,
    ch: char,
    color: Color,
    scrolls: bool,
}
enum SceneryKind {
    Tree,
    Bush,
    Rock,
    Sign(usize),
}
struct Scenery {
    x: i32,
    y: f64,
    kind: SceneryKind,
}
struct FloatText {
    text: String,
    x: f64,
    y: f64,
    t: f64,
    color: Color,
}

// ── input (press/release tracking with legacy-terminal fallback) ─────────
#[derive(Default)]
struct Input {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    nitro: bool,
    left_t: f64,
    right_t: f64,
    up_t: f64,
    down_t: f64,
    nitro_t: f64,
    release_seen: bool,
}

impl Input {
    fn press(&mut self, code: KeyCode, now: f64) {
        let t = now + 0.25;
        match code {
            KeyCode::Left | KeyCode::Char('a') => {
                self.left = true;
                self.left_t = t;
            }
            KeyCode::Right | KeyCode::Char('d') => {
                self.right = true;
                self.right_t = t;
            }
            KeyCode::Up | KeyCode::Char('w') => {
                self.up = true;
                self.up_t = t;
            }
            KeyCode::Down | KeyCode::Char('s') => {
                self.down = true;
                self.down_t = t;
            }
            KeyCode::Char(' ') => {
                self.nitro = true;
                self.nitro_t = t;
            }
            _ => {}
        }
    }
    fn release(&mut self, code: KeyCode) {
        self.release_seen = true;
        match code {
            KeyCode::Left | KeyCode::Char('a') => self.left = false,
            KeyCode::Right | KeyCode::Char('d') => self.right = false,
            KeyCode::Up | KeyCode::Char('w') => self.up = false,
            KeyCode::Down | KeyCode::Char('s') => self.down = false,
            KeyCode::Char(' ') => self.nitro = false,
            _ => {}
        }
    }
    fn held(&self, flag: bool, timer: f64, now: f64) -> bool {
        if self.release_seen { flag } else { now < timer }
    }
    fn steer(&self, now: f64) -> f64 {
        let r = self.held(self.right, self.right_t, now) as i32;
        let l = self.held(self.left, self.left_t, now) as i32;
        (r - l) as f64
    }
}

// ── geometry (shared by simulation & renderer) ────────────────────────────
struct Geom {
    lane_w: i32,
    rx: i32, // first asphalt column
    road_w: i32,
}
fn geom(view: Rect) -> Geom {
    let lane_w = ((((view.width as i32) - 8) / 4) - 1).clamp(4, 8);
    let road_w = lane_w * 4;
    let total = road_w + 4;
    let rx = ((view.width as i32) - total) / 2 + 2;
    Geom { lane_w, rx, road_w }
}
fn lane_center(g: &Geom, lane: usize) -> f64 {
    g.lane_w as f64 * (lane as f64 + 0.5)
}
fn player_y(view: Rect) -> i32 {
    view.height as i32 - 8
}

fn puff(
    particles: &mut Vec<Particle>,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    life: f64,
    ch: char,
    color: Color,
    scrolls: bool,
) {
    if particles.len() > 320 {
        particles.drain(0..40);
    }
    particles.push(Particle {
        x,
        y,
        vx,
        vy,
        life,
        max: life,
        ch,
        color,
        scrolls,
    });
}

fn beep(sound: bool, n: usize) {
    if !sound {
        return;
    }
    let mut out = io::stdout();
    for _ in 0..n {
        let _ = write!(out, "\x07");
    }
    let _ = out.flush();
}

// ── the game ──────────────────────────────────────────────────────────────
struct Game {
    state: State,
    diff: Difficulty,
    sel: Difficulty,
    time: f64,
    run_t: f64,
    countdown: f64,
    last_count: i32,
    crash_t: f64,
    speed: f64,
    top_speed: f64,
    scroll: f64,
    distance: f64,
    score: f64,
    hi: [u64; 3],
    new_best: bool,
    nitro: f64,
    nitro_on: bool,
    player_x: f64,
    player_vx: f64,
    traffic: Vec<Car>,
    pickups: Vec<Pickup>,
    particles: Vec<Particle>,
    scenery: Vec<Scenery>,
    floats: Vec<FloatText>,
    combo: u32,
    combo_t: f64,
    best_combo: u32,
    near_misses: u32,
    shake: f64,
    spawn_t: f64,
    pickup_t: f64,
    scenery_t: f64,
    exhaust_t: f64,
    dust_t: f64,
    wreck_t: f64,
    offroad_msg_t: f64,
    quip: usize,
    sound: bool,
    rng: ThreadRng,
}

impl Game {
    fn new(hi: [u64; 3]) -> Self {
        Game {
            state: State::Menu,
            diff: Difficulty::Rush,
            sel: Difficulty::Rush,
            time: 0.0,
            run_t: 0.0,
            countdown: 0.0,
            last_count: 99,
            crash_t: 0.0,
            speed: 0.0,
            top_speed: 0.0,
            scroll: 0.0,
            distance: 0.0,
            score: 0.0,
            hi,
            new_best: false,
            nitro: 1.0,
            nitro_on: false,
            player_x: 16.0,
            player_vx: 0.0,
            traffic: Vec::new(),
            pickups: Vec::new(),
            particles: Vec::new(),
            scenery: Vec::new(),
            floats: Vec::new(),
            combo: 0,
            combo_t: 0.0,
            best_combo: 0,
            near_misses: 0,
            shake: 0.0,
            spawn_t: 1.0,
            pickup_t: 5.0,
            scenery_t: 0.2,
            exhaust_t: 0.0,
            dust_t: 0.0,
            wreck_t: 0.0,
            offroad_msg_t: 0.0,
            quip: 0,
            sound: true,
            rng: rand::rng(),
        }
    }

    fn start_run(&mut self, d: Difficulty, view: Rect) {
        let g = geom(view);
        self.diff = d;
        self.state = State::Countdown;
        self.countdown = 2.999;
        self.last_count = 99;
        self.run_t = 0.0;
        self.crash_t = 0.0;
        self.speed = 0.0;
        self.top_speed = 0.0;
        self.distance = 0.0;
        self.score = 0.0;
        self.new_best = false;
        self.nitro = 1.0;
        self.nitro_on = false;
        self.player_x = g.road_w as f64 / 2.0;
        self.player_vx = 0.0;
        self.combo = 0;
        self.combo_t = 0.0;
        self.best_combo = 0;
        self.near_misses = 0;
        self.shake = 0.0;
        self.traffic.clear();
        self.pickups.clear();
        self.particles.clear();
        self.floats.clear();
        self.spawn_t = 0.8;
        self.pickup_t = 4.0;
        self.offroad_msg_t = 0.0;
    }

    fn to_menu(&mut self) {
        self.state = State::Menu;
        self.traffic.clear();
        self.pickups.clear();
        self.particles.clear();
        self.floats.clear();
        self.speed = 0.0;
        self.shake = 0.0;
        self.nitro_on = false;
    }

    fn flash(&mut self, text: &str, color: Color, view: Rect) {
        self.floats.push(FloatText {
            text: text.to_string(),
            x: self.player_x,
            y: player_y(view) as f64 - 3.0,
            t: 0.0,
            color,
        });
    }

    fn explode(&mut self, x: f64, y: f64) {
        let cols = [
            Color::Rgb(255, 120, 40),
            Color::Rgb(255, 200, 60),
            Color::Rgb(255, 70, 40),
            Color::Rgb(240, 240, 240),
            Color::Rgb(130, 130, 135),
        ];
        let chars = ['█', '▓', '▒', '░', '*'];
        for _ in 0..44 {
            let a: f64 = self.rng.random_range(0.0..std::f64::consts::TAU);
            let sp: f64 = self.rng.random_range(5.0..32.0);
            let jx = self.rng.random_range(-1.5..1.5);
            let jy = self.rng.random_range(-1.0..2.0);
            let life = self.rng.random_range(0.5..1.2);
            let ch = chars[self.rng.random_range(0..chars.len())];
            let col = cols[self.rng.random_range(0..cols.len())];
            puff(
                &mut self.particles,
                x + jx,
                y + jy,
                a.cos() * sp,
                a.sin() * sp - 5.0,
                life,
                ch,
                col,
                true,
            );
        }
        for _ in 0..12 {
            let a: f64 = self.rng.random_range(0.0..std::f64::consts::TAU);
            let sp: f64 = self.rng.random_range(2.0..10.0);
            puff(
                &mut self.particles,
                x,
                y + 1.0,
                a.cos() * sp,
                a.sin() * sp - 3.0,
                1.0,
                '▓',
                Color::Rgb(55, 52, 50),
                true,
            );
        }
    }

    fn crash(&mut self, view: Rect) {
        self.state = State::Crashing;
        self.crash_t = 0.0;
        self.wreck_t = 0.0;
        self.shake = 1.3;
        self.nitro_on = false;
        self.combo = 0;
        self.quip = self.rng.random_range(0..QUIPS.len());
        self.explode(self.player_x, player_y(view) as f64 + 1.5);
        beep(self.sound, 3);
    }

    fn commit_hi(&mut self) {
        let s = self.score as u64;
        if s > self.hi[self.diff.index()] {
            self.hi[self.diff.index()] = s;
            self.new_best = true;
            save_hi(&self.hi);
        }
    }

    // ── simulation ────────────────────────────────────────────────────────
    fn update(&mut self, dt: f64, view: Rect, input: &Input) {
        self.time += dt;
        match self.state {
            State::Menu => self.update_attract(dt, view),
            State::Countdown => self.update_countdown(dt, view),
            State::Running => self.update_running(dt, view, input),
            State::Crashing => self.update_crashing(dt, view),
            State::GameOver => self.update_gameover(dt, view),
            State::Paused => {}
        }
    }

    fn update_particles(&mut self, dt: f64, rows: f64, view: Rect) {
        for pt in &mut self.particles {
            pt.x += pt.vx * dt;
            pt.y += pt.vy * dt;
            if pt.scrolls {
                pt.y += rows;
            }
            pt.life -= dt;
        }
        self.particles
            .retain(|p| p.life > 0.0 && p.y < view.height as f64 + 6.0 && p.y > -8.0);
        for ft in &mut self.floats {
            ft.t += dt;
            ft.y -= 7.0 * dt;
        }
        self.floats.retain(|f| f.t < 1.15);
    }

    fn update_scenery_spawn(&mut self, dt: f64, view: Rect) {
        self.scenery_t -= dt;
        if self.scenery_t > 0.0 {
            return;
        }
        self.scenery_t = self.rng.random_range(0.18..0.55);
        let g = geom(view);
        let left_w = g.rx - 2;
        let right_x = g.rx + g.road_w + 3;
        let right_w = view.width as i32 - right_x - 1;
        let side_left = self.rng.random_bool(0.5);
        let roll: f64 = self.rng.random();
        let kind = if roll < 0.42 {
            SceneryKind::Tree
        } else if roll < 0.66 {
            SceneryKind::Bush
        } else if roll < 0.82 {
            SceneryKind::Rock
        } else {
            let si = self.rng.random_range(0..SIGNS.len());
            let need = SIGNS[si].len() as i32 + 6;
            if (side_left && left_w < need) || (!side_left && right_w < need) {
                SceneryKind::Tree
            } else {
                SceneryKind::Sign(si)
            }
        };
        let span = if side_left { left_w } else { right_w };
        if span < 4 {
            return;
        }
        let x = if side_left {
            self.rng.random_range(1..(span - 3).max(2))
        } else {
            right_x + self.rng.random_range(0..(span - 4).max(1))
        };
        self.scenery.push(Scenery { x, y: -8.0, kind });
    }

    fn update_scenery_move(&mut self, dt: f64, rows: f64, view: Rect) {
        let _ = dt;
        for s in &mut self.scenery {
            s.y += rows;
        }
        self.scenery.retain(|s| s.y < view.height as f64 + 10.0);
    }

    fn spawn_traffic(&mut self, d: Difficulty, view: Rect) {
        let g = geom(view);
        let blocked: Vec<usize> = {
            let mut v: Vec<usize> = self
                .traffic
                .iter()
                .filter(|c| c.y < 20.0)
                .map(|c| c.lane)
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        if blocked.len() >= 3 {
            return; // always leave an escape lane
        }
        let free: Vec<usize> = (0..4).filter(|l| !blocked.contains(l)).collect();
        if free.is_empty() {
            return;
        }
        let lane = free[self.rng.random_range(0..free.len())];
        if self.traffic.iter().any(|c| c.lane == lane && c.y < 16.0) {
            return;
        }
        let truck = self.rng.random_bool(0.22);
        let (a, b) = d.traffic();
        let mut speed: f64 = self.rng.random_range(a..b);
        if truck {
            speed *= 0.8;
        }
        self.traffic.push(Car {
            lane,
            x: lane_center(&g, lane),
            y: -10.0,
            speed,
            color: TRAFFIC_COLORS[self.rng.random_range(0..TRAFFIC_COLORS.len())],
            truck,
            passed: false,
            wob: self.rng.random_range(0.0..6.28),
            wob_f: self.rng.random_range(0.7..1.9),
        });
    }

    fn update_attract(&mut self, dt: f64, view: Rect) {
        self.speed += (88.0 - self.speed) * dt * 0.7;
        let rows = self.speed * K_ROWS * dt;
        self.scroll += rows;
        let g = geom(view);
        self.player_x = g.road_w as f64 / 2.0 + (self.time * 0.8).sin() * 3.0;
        self.update_scenery_spawn(dt, view);
        self.update_scenery_move(dt, rows, view);
        self.spawn_t -= dt;
        if self.spawn_t <= 0.0 {
            self.spawn_traffic(self.sel, view);
            self.spawn_t = self.rng.random_range(1.8..3.2);
        }
        for c in &mut self.traffic {
            c.y += (self.speed - c.speed) * K_ROWS * dt;
        }
        self.traffic
            .retain(|c| c.y < view.height as f64 + 12.0 && c.y > -40.0);
        self.exhaust_t -= dt;
        if self.exhaust_t <= 0.0 {
            self.exhaust_t = 0.06;
            let py = player_y(view) as f64;
            let jx = self.rng.random_range(-0.7..0.7);
            let jvx = self.rng.random_range(-1.5..1.5);
            puff(
                &mut self.particles,
                self.player_x + jx,
                py + 5.0,
                jvx,
                8.0,
                0.45,
                '░',
                Color::Rgb(110, 110, 116),
                true,
            );
        }
        self.update_particles(dt, rows, view);
    }

    fn update_countdown(&mut self, dt: f64, view: Rect) {
        self.countdown -= dt;
        self.speed = (self.speed + 34.0 * dt).min(42.0);
        let rows = self.speed * K_ROWS * dt;
        self.scroll += rows;
        self.update_scenery_spawn(dt, view);
        self.update_scenery_move(dt, rows, view);
        self.update_particles(dt, rows, view);
        let n = self.countdown.ceil() as i32;
        if n != self.last_count {
            self.last_count = n;
            beep(self.sound, 1);
        }
        if self.countdown <= 0.0 {
            self.state = State::Running;
            self.run_t = 0.0;
            beep(self.sound, 2);
        }
    }

    fn update_running(&mut self, dt: f64, view: Rect, input: &Input) {
        self.run_t += dt;
        let g = geom(view);
        let road_w = g.road_w as f64;
        let now = self.time;
        let py = player_y(view) as f64;

        // steering
        let steer = input.steer(now);
        if steer != 0.0 {
            self.player_vx += steer * 130.0 * dt;
        } else {
            self.player_vx *= (1.0 - 10.0 * dt).clamp(0.0, 1.0);
        }
        self.player_vx = self.player_vx.clamp(-34.0, 34.0);
        self.player_x = (self.player_x + self.player_vx * dt).clamp(-1.2, road_w + 1.2);
        if self.player_x <= -1.19 || self.player_x >= road_w + 1.19 {
            self.player_vx = 0.0;
        }
        let offroad = self.player_x < 1.6 || self.player_x > road_w - 1.6;

        // nitro
        let want = input.held(input.nitro, input.nitro_t, now) && self.nitro > 0.02;
        if want && !self.nitro_on {
            self.flash("NITRO!", Color::Rgb(120, 230, 255), view);
            beep(self.sound, 1);
        }
        self.nitro_on = want;
        if self.nitro_on {
            self.nitro = (self.nitro - 0.32 * dt).max(0.0);
            self.shake = self.shake.max(0.12);
        } else {
            self.nitro = (self.nitro + 0.055 * dt).min(1.0);
        }

        // speed
        let max = self.diff.max_speed() + if self.nitro_on { 55.0 } else { 0.0 };
        let accel = if input.held(input.down, input.down_t, now) {
            -130.0
        } else if input.held(input.up, input.up_t, now) {
            62.0
        } else {
            17.0
        };
        self.speed = (self.speed + accel * dt).clamp(32.0, max);
        if offroad {
            self.speed = (self.speed - 85.0 * dt).max(55.0);
            self.shake = self.shake.max(0.3);
            self.dust_t -= dt;
            if self.dust_t <= 0.0 {
                self.dust_t = 0.06;
                for dx in [-2.0, 2.0] {
                    let jvx = self.rng.random_range(-3.0..3.0);
                    puff(
                        &mut self.particles,
                        self.player_x + dx,
                        py + 4.0,
                        jvx,
                        5.0,
                        0.4,
                        '░',
                        Color::Rgb(165, 130, 70),
                        true,
                    );
                }
            }
            self.offroad_msg_t -= dt;
            if self.offroad_msg_t <= 0.0 {
                self.offroad_msg_t = 1.3;
                self.flash("OFF ROAD!", Color::Rgb(255, 170, 80), view);
            }
        }
        self.top_speed = self.top_speed.max(self.speed);

        let rows = self.speed * K_ROWS * dt;
        self.scroll += rows;
        self.distance += self.speed / 3.6 * dt;
        let mult = 1.0 + 0.25 * self.combo as f64;
        self.score += self.speed * dt * 0.55 * mult * if self.nitro_on { 1.5 } else { 1.0 };

        if self.combo > 0 {
            self.combo_t -= dt;
            if self.combo_t <= 0.0 {
                self.combo = 0;
            }
        }

        // traffic
        for c in &mut self.traffic {
            c.y += (self.speed - c.speed) * K_ROWS * dt;
            c.wob += dt * c.wob_f;
            c.x = lane_center(&g, c.lane) + c.wob.sin() * 0.22;
        }
        let mut crashed = false;
        for c in &mut self.traffic {
            let ch = if c.truck { 7.0 } else { 5.0 };
            let dx = (self.player_x - c.x).abs();
            let overlap_y = c.y < py + 5.0 - 0.7 && c.y + ch > py + 0.7;
            if overlap_y && dx < 4.3 {
                crashed = true;
            }
            if !c.passed && c.y > py + 5.0 {
                c.passed = true;
                let gap = dx - 4.6;
                if gap < 1.8 && self.speed > c.speed + 5.0 {
                    self.combo += 1;
                    self.combo_t = 2.6;
                    self.best_combo = self.best_combo.max(self.combo);
                    self.near_misses += 1;
                    let bonus = 100 + 60 * self.combo;
                    self.score += bonus as f64;
                    let msg = if self.combo > 1 {
                        format!("+{} CLOSE! x{}", bonus, self.combo)
                    } else {
                        format!("+{} CLOSE!", bonus)
                    };
                    self.floats.push(FloatText {
                        text: msg,
                        x: c.x,
                        y: py - 2.0,
                        t: 0.0,
                        color: Color::Rgb(255, 225, 90),
                    });
                    for _ in 0..6 {
                        let svx = self.rng.random_range(-14.0..14.0);
                        let svy = self.rng.random_range(-10.0..10.0);
                        puff(
                            &mut self.particles,
                            (self.player_x + c.x) / 2.0,
                            py + 1.0,
                            svx,
                            svy,
                            0.3,
                            '*',
                            Color::Rgb(255, 240, 150),
                            true,
                        );
                    }
                    beep(self.sound, 1);
                }
            }
        }
        if crashed {
            self.crash(view);
            return;
        }
        self.traffic
            .retain(|c| c.y < view.height as f64 + 12.0 && c.y > -60.0);

        // pickups
        let mut collected = Vec::new();
        for (i, pk) in self.pickups.iter().enumerate() {
            if (pk.x - self.player_x).abs() < 3.0 && (pk.y - (py + 2.0)).abs() < 3.2 {
                collected.push(i);
            }
        }
        for &i in collected.iter().rev() {
            let pk = self.pickups[i];
            self.pickups.swap_remove(i);
            self.score += 250.0;
            self.nitro = (self.nitro + 0.35).min(1.0);
            self.floats.push(FloatText {
                text: "+250 ◆".to_string(),
                x: pk.x,
                y: pk.y,
                t: 0.0,
                color: Color::Rgb(255, 205, 60),
            });
            // ── pickup sparkle (replace the loop body) ──
            for _ in 0..10 {
                let a: f64 = self.rng.random_range(0.0..std::f64::consts::TAU);
                puff(
                    &mut self.particles,
                    pk.x,
                    pk.y,
                    a.cos() * 12.0,
                    a.sin() * 12.0,
                    0.4,
                    '*',
                    Color::Rgb(255, 215, 90),
                    true,
                );
            }
            beep(self.sound, 2);
        }
        for pk in &mut self.pickups {
            pk.y += rows;
            pk.t += dt;
        }
        self.pickups.retain(|pk| pk.y < view.height as f64 + 6.0);

        // spawns
        self.spawn_t -= dt;
        if self.spawn_t <= 0.0 {
            self.spawn_traffic(self.diff, view);
            let (a, b) = self.diff.spawn_gap();
            let density = (1.45 - 0.65 * (self.speed / self.diff.max_speed())).clamp(0.6, 1.5);
            self.spawn_t = self.rng.random_range(a..b) * density;
        }
        self.pickup_t -= dt;
        if self.pickup_t <= 0.0 {
            self.pickup_t = self.rng.random_range(4.0..8.0);
            let lane = self.rng.random_range(0..4);
            if !self.traffic.iter().any(|c| c.lane == lane && c.y < 12.0) {
                self.pickups.push(Pickup {
                    x: lane_center(&g, lane),
                    y: -4.0,
                    t: 0.0,
                });
            }
        }
        self.update_scenery_spawn(dt, view);
        self.update_scenery_move(dt, rows, view);

        // exhaust / flames
        self.exhaust_t -= dt;
        if self.exhaust_t <= 0.0 {
            self.exhaust_t = if self.nitro_on { 0.03 } else { 0.05 };
            // ── nitro flames ──
            if self.nitro_on {
                for _ in 0..2 {
                    let jx = self.rng.random_range(-1.0..1.0);
                    let jvx = self.rng.random_range(-3.0..3.0);
                    let jvy = self.rng.random_range(16.0..24.0);
                    let life = self.rng.random_range(0.18..0.3);
                    let ch = ['▓', '▒', '░'][self.rng.random_range(0..3)];
                    let col = [
                        Color::Rgb(120, 230, 255),
                        Color::Rgb(210, 245, 255),
                        Color::Rgb(80, 160, 255),
                    ][self.rng.random_range(0..3)];
                    puff(
                        &mut self.particles,
                        self.player_x + jx,
                        py + 4.8,
                        jvx,
                        jvy,
                        life,
                        ch,
                        col,
                        true,
                    );
                }
            } else {
                let jx = self.rng.random_range(-0.7..0.7);
                let jvx = self.rng.random_range(-1.5..1.5);
                let life = self.rng.random_range(0.35..0.55);
                let ch = if self.rng.random_bool(0.5) {
                    '░'
                } else {
                    '▒'
                };
                puff(
                    &mut self.particles,
                    self.player_x + jx,
                    py + 5.0,
                    jvx,
                    self.rng.random_range(7.0..10.0),
                    life,
                    ch,
                    Color::Rgb(105, 105, 112),
                    true,
                );
            }
        }
        self.update_particles(dt, rows, view);
        self.shake = (self.shake - 2.2 * dt).max(0.0);
    }

    fn update_crashing(&mut self, dt: f64, view: Rect) {
        self.crash_t += dt;
        self.speed = (self.speed - 300.0 * dt).max(0.0);
        let rows = self.speed * K_ROWS * dt;
        self.scroll += rows;
        self.update_scenery_move(dt, rows, view);
        self.wreck_t -= dt;
        if self.wreck_t <= 0.0 {
            self.wreck_t = 0.09;
            let py = player_y(view) as f64;
            let jx = self.rng.random_range(-1.5..1.5);
            let jy = self.rng.random_range(0.0..3.0);
            let ch = if self.rng.random_bool(0.6) {
                '░'
            } else {
                '▒'
            };
            puff(
                &mut self.particles,
                self.player_x + jx,
                py + jy,
                self.rng.random_range(-2.0..2.0),
                -4.0,
                0.9,
                ch,
                Color::Rgb(90, 90, 96),
                true,
            );
            if self.rng.random_bool(0.4) {
                let ex = self.rng.random_range(-1.5..1.5);
                puff(
                    &mut self.particles,
                    self.player_x + ex,
                    py + 2.0,
                    0.0,
                    -2.0,
                    0.5,
                    '*',
                    Color::Rgb(255, 140, 50),
                    true,
                );
            }
        }
        self.update_particles(dt, rows, view);
        self.shake = (self.shake - 1.4 * dt).max(0.0);
        if self.crash_t > 1.5 {
            self.state = State::GameOver;
            self.commit_hi();
        }
    }

    fn update_gameover(&mut self, dt: f64, view: Rect) {
        self.wreck_t -= dt;
        if self.wreck_t <= 0.0 {
            self.wreck_t = 0.16;
            let py = player_y(view) as f64;
            let jx = self.rng.random_range(-1.5..1.5);
            puff(
                &mut self.particles,
                self.player_x + jx,
                py + 1.0,
                0.0,
                -3.5,
                1.0,
                '░',
                Color::Rgb(80, 80, 86),
                true,
            );
        }
        self.update_particles(dt, 0.0, view);
        self.shake = (self.shake - 2.0 * dt).max(0.0);
    }
}

// ── painting helpers ──────────────────────────────────────────────────────
struct Painter<'a> {
    buf: &'a mut Buffer,
    ox: i32,
    oy: i32,
}
impl<'a> Painter<'a> {
    fn new(buf: &'a mut Buffer, off: (i32, i32)) -> Self {
        Painter {
            buf,
            ox: off.0,
            oy: off.1,
        }
    }
    fn put(&mut self, x: i32, y: i32, ch: char, style: Style) {
        let px = x + self.ox;
        let py = y + self.oy;
        if px < 0 || py < 0 || px > u16::MAX as i32 || py > u16::MAX as i32 {
            return;
        }
        if let Some(cell) = self.buf.cell_mut(Position {
            x: px as u16,
            y: py as u16,
        }) {
            cell.set_char(ch);
            cell.set_style(style);
        }
    }
    fn put_str(&mut self, x: i32, y: i32, s: &str, style: Style) {
        for (i, ch) in s.chars().enumerate() {
            if ch != ' ' {
                self.put(x + i as i32, y, ch, style);
            }
        }
    }
}

fn hash2(x: i32, wy: i64) -> u32 {
    let mut h = (x as i64).wrapping_mul(374_761_393) ^ wy.wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0x7fff_ffff) as u32
}
fn grass_char(x: i32, wy: i64) -> char {
    match hash2(x, wy) % 29 {
        0 => '\'',
        1 => '.',
        2 => ',',
        3 => '`',
        _ => ' ',
    }
}

fn draw_car(
    p: &mut Painter,
    x: i32,
    y: i32,
    rows: &[&str],
    body: Color,
    glass: Color,
    lights: Option<(usize, Color)>,
) {
    let last = rows.len() - 1;
    for (r, row) in rows.iter().enumerate() {
        for (i, ch) in row.chars().enumerate() {
            if ch == ' ' {
                continue;
            }
            let mut st = match ch {
                '▐' | '▌' => Style::default().fg(Color::Rgb(14, 14, 18)),
                '▓' => Style::default().fg(glass),
                '░' => Style::default().fg(Color::Rgb(150, 150, 162)),
                _ => Style::default().fg(body),
            };
            if let Some((lr, lc)) = lights {
                if r == lr && (i == 1 || i == 3) && (ch == '▄' || ch == '▀') {
                    st = Style::default().fg(lc);
                }
            }
            let _ = last;
            p.put(x + i as i32, y + r as i32, ch, st);
        }
    }
}

// ── world renderer ────────────────────────────────────────────────────────
fn render_world(p: &mut Painter, g: &Game, view: Rect) {
    let gm = geom(view);
    let w = view.width as i32;
    let h = view.height as i32;
    let scroll_i = g.scroll.floor() as i64;
    let grass1 = Color::Rgb(14, 40, 22);
    let grass2 = Color::Rgb(19, 52, 28);
    let tex = Color::Rgb(46, 92, 48);
    let asphalt = Color::Rgb(33, 33, 40);
    let asphalt2 = Color::Rgb(27, 27, 33);
    let dash = Color::Rgb(215, 215, 225);
    let edge = Color::Rgb(235, 235, 240);
    let fast = g.speed > g.diff.max_speed() * 0.8 || g.nitro_on;

    // terrain
    for yi in 0..h {
        let wy = scroll_i + yi as i64;
        let stripe = ((wy / 5) % 2 + 2) % 2 == 0;
        let gbg = if stripe { grass1 } else { grass2 };
        for xi in 0..w {
            if xi < gm.rx - 2 || xi >= gm.rx + gm.road_w + 2 {
                let mut ch = grass_char(xi, wy);
                let mut fg = tex;
                if fast && hash2(xi * 7 + 3, wy) % 23 == 0 {
                    ch = '│';
                    fg = Color::Rgb(150, 235, 165);
                }
                p.put(xi, yi, ch, Style::default().fg(fg).bg(gbg));
            } else if xi < gm.rx || xi >= gm.rx + gm.road_w {
                let c = if ((wy / 2) % 2 + 2) % 2 == 0 {
                    Color::Rgb(205, 60, 55)
                } else {
                    Color::Rgb(235, 235, 235)
                };
                p.put(xi, yi, '█', Style::default().fg(c).bg(c));
            } else if xi == gm.rx || xi == gm.rx + gm.road_w - 1 {
                p.put(xi, yi, ' ', Style::default().bg(edge));
            } else {
                let bg = if hash2(xi, wy) % 41 == 0 {
                    asphalt2
                } else {
                    asphalt
                };
                let mut st = Style::default().bg(bg);
                if g.nitro_on && hash2(xi * 13 + 5, wy) % 11 == 0 {
                    st = Style::default().fg(Color::Rgb(120, 210, 255)).bg(bg);
                    p.put(xi, yi, '╎', st);
                    continue;
                }
                p.put(xi, yi, ' ', st);
            }
        }
        for i in 1..4 {
            let x = gm.rx + gm.lane_w * i;
            if ((wy % 4) + 4) % 4 < 2 {
                p.put(x, yi, ' ', Style::default().bg(dash));
            }
        }
    }

    // scenery
    for s in &g.scenery {
        let y = s.y.round() as i32;
        match s.kind {
            SceneryKind::Tree => {
                p.put_str(
                    s.x,
                    y,
                    " ▄█▄ ",
                    Style::default().fg(Color::Rgb(52, 150, 64)),
                );
                p.put_str(
                    s.x,
                    y + 1,
                    "▐███▌",
                    Style::default().fg(Color::Rgb(38, 120, 50)),
                );
                p.put_str(
                    s.x,
                    y + 2,
                    " ▀▄▀ ",
                    Style::default().fg(Color::Rgb(30, 95, 42)),
                );
            }
            SceneryKind::Bush => {
                p.put_str(s.x, y, "▄▓▄", Style::default().fg(Color::Rgb(40, 110, 48)));
            }
            SceneryKind::Rock => {
                p.put_str(
                    s.x,
                    y,
                    "▄▓▄",
                    Style::default().fg(Color::Rgb(115, 115, 122)),
                );
                p.put_str(
                    s.x,
                    y + 1,
                    "▀▀▀",
                    Style::default().fg(Color::Rgb(85, 85, 92)),
                );
            }
            SceneryKind::Sign(si) => {
                let msg = SIGNS[si];
                let bw = msg.len() as i32 + 4;
                let yellow = Style::default().fg(Color::Rgb(240, 190, 60));
                for i in 0..bw {
                    p.put(s.x + i, y, '▄', yellow);
                    p.put(s.x + i, y + 2, '▀', yellow);
                }
                p.put(s.x, y + 1, '█', yellow);
                p.put(s.x + bw - 1, y + 1, '█', yellow);
                for i in 0..bw - 2 {
                    let ch = msg.chars().nth(i as usize).unwrap_or(' ');
                    p.put(
                        s.x + 1 + i,
                        y + 1,
                        ch,
                        Style::default()
                            .fg(Color::Rgb(240, 240, 245))
                            .bg(Color::Rgb(24, 44, 96)),
                    );
                }
                p.put(
                    s.x + 1,
                    y + 3,
                    '█',
                    Style::default().fg(Color::Rgb(90, 90, 96)),
                );
                p.put(
                    s.x + bw - 2,
                    y + 3,
                    '█',
                    Style::default().fg(Color::Rgb(90, 90, 96)),
                );
            }
        }
    }

    // pickups
    for pk in &g.pickups {
        let x = pk.x.round() as i32;
        let y = pk.y.round() as i32;
        let solid = (pk.t * 5.0) as i64 % 2 == 0;
        let st = Style::default()
            .fg(Color::Rgb(255, 205, 60))
            .bg(Color::Rgb(60, 50, 15))
            .add_modifier(Modifier::BOLD);
        p.put(x, y, if solid { '◆' } else { '◇' }, st);
        p.put(x - 1, y, '·', Style::default().fg(Color::Rgb(150, 120, 40)));
        p.put(x + 1, y, '·', Style::default().fg(Color::Rgb(150, 120, 40)));
    }

    // traffic
    for c in &g.traffic {
        let x = gm.rx + c.x.round() as i32 - 2;
        let y = c.y.round() as i32;
        if c.truck {
            draw_car(
                p,
                x,
                y,
                &TRUCK,
                c.color,
                Color::Rgb(120, 170, 210),
                Some((6, Color::Rgb(255, 70, 55))),
            );
        } else {
            draw_car(
                p,
                x,
                y,
                &SEDAN,
                c.color,
                Color::Rgb(120, 170, 210),
                Some((4, Color::Rgb(255, 70, 55))),
            );
        }
    }

    // player (or wreck)
    let px = gm.rx + g.player_x.round() as i32 - 2;
    let py = player_y(view);
    match g.state {
        State::Crashing | State::GameOver => {
            draw_car(
                p,
                px,
                py,
                &SEDAN,
                Color::Rgb(58, 55, 52),
                Color::Rgb(82, 82, 86),
                None,
            );
        }
        _ => {
            draw_car(
                p,
                px,
                py,
                &SEDAN,
                Color::Rgb(255, 96, 60),
                Color::Rgb(150, 210, 250),
                Some((0, Color::Rgb(255, 240, 150))),
            );
        }
    }

    // particles
    for pt in &g.particles {
        let mut st = Style::default().fg(pt.color);
        if pt.life / pt.max < 0.4 {
            st = st.add_modifier(Modifier::DIM);
        }
        p.put(pt.x.round() as i32, pt.y.round() as i32, pt.ch, st);
    }

    // floating score text
    for ft in &g.floats {
        let mut st = Style::default().fg(ft.color).add_modifier(Modifier::BOLD);
        if ft.t > 0.55 {
            st = st.add_modifier(Modifier::DIM);
        }
        let x = (gm.rx as f64 + ft.x - ft.text.len() as f64 / 2.0).round() as i32;
        p.put_str(x, ft.y.round() as i32, &ft.text, st);
    }
}

// ── block-letter glyphs ───────────────────────────────────────────────────
fn glyph(c: char) -> [&'static str; 3] {
    match c {
        'R' => ["█▀▀█", "█▄▄▀", "▀ ▀▀"],
        'A' => ["▄▀▀▄", "█▄▄█", "▀  ▀"],
        'T' => ["▄█▄", " █ ", " ▀ "],
        'C' => ["▄▀▀▀", "█░░ ", "▀▀▀▀"],
        'E' => ["█▀▀▀", "█▄▄ ", "▀▀▀▀"],
        'G' => ["▄▀▀▀", "█░░█", "▀▀▀█"],
        'O' => ["▄▀▀▄", "█░░█", "▀▀▀▀"],
        'W' => ["█   █", "█▄█▄█", " ▀ ▀ "],
        'K' => ["█ ▄▀", "█▄▄ ", "▀ ▀▀"],
        'D' => ["█▀▀▄", "█░░█", "▀▀▀▀"],
        'P' => ["█▀▀█", "█▄▄▀", "▀   "],
        'U' => ["█  █", "█  █", "▀▀▀▀"],
        'S' => ["▄▀▀▀", "▀▀▀▄", "▀▀▀▀"],
        '1' => ["▄█ ", " █ ", "▀▀▀"],
        '2' => ["▄▀▀▄", "░▄▀░", "▀▀▀▀"],
        '3' => ["▄▀▀▀", "░▄▄▀", "▀▀▀▀"],
        '!' => ["█", " ", "▀"],
        _ => [" ", " ", " "],
    }
}
fn big_text(s: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for (ci, ch) in s.chars().enumerate() {
        if ci > 0 {
            for r in &mut rows {
                r.push(' ');
            }
        }
        let gl = glyph(ch);
        for i in 0..3 {
            rows[i].push_str(gl[i]);
        }
    }
    rows
}
fn draw_big(
    p: &mut Painter,
    cx: i32,
    y: i32,
    rows: &[String; 3],
    style: Style,
    shadow: Option<Style>,
) {
    let w = rows[0].chars().count() as i32;
    let x = cx - w / 2;
    for (i, row) in rows.iter().enumerate() {
        if let Some(sh) = shadow {
            p.put_str(x + 1, y + i as i32 + 1, row, sh);
        }
        p.put_str(x, y + i as i32, row, style);
    }
}

// ── HUD / footer widgets ──────────────────────────────────────────────────
fn blink(t: f64, hz: f64) -> bool {
    (t * hz) % 1.0 < 0.62
}
fn speed_color(frac: f64) -> Color {
    if frac < 0.55 {
        Color::Rgb(90, 230, 120)
    } else if frac < 0.85 {
        Color::Rgb(255, 210, 80)
    } else {
        Color::Rgb(255, 90, 70)
    }
}
fn gauge(frac: f64, n: usize, on: Color) -> Vec<Span<'static>> {
    let filled = (frac.clamp(0.0, 1.0) * n as f64).round() as usize;
    vec![
        Span::styled("█".repeat(filled), Style::default().fg(on)),
        Span::styled(
            "░".repeat(n - filled),
            Style::default().fg(Color::Rgb(58, 58, 68)),
        ),
    ]
}

struct Hud<'a>(&'a Game);
impl Widget for Hud<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let g = self.0;
        let block = Block::bordered()
            .border_type(BorderType::Plain)
            .border_style(Style::default().fg(Color::Rgb(64, 64, 76)))
            .style(Style::default().bg(Color::Rgb(12, 12, 16)));
        let inner = block.inner(area);
        block.render(area, buf);
        let lbl = Style::default().fg(Color::Rgb(118, 118, 134));
        let mut spans: Vec<Span> = vec![Span::styled(" SCORE ", lbl)];
        spans.push(Span::styled(
            format!("{:06}", (g.score as u64).min(999_999)),
            Style::default()
                .fg(Color::Rgb(245, 245, 250))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("   HI ", lbl));
        spans.push(Span::styled(
            format!("{:06}", g.hi[g.diff.index()]),
            Style::default()
                .fg(Color::Rgb(255, 205, 70))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("   SPD ", lbl));
        let frac = g.speed / g.diff.max_speed();
        spans.extend(gauge(frac, 8, speed_color(frac)));
        spans.push(Span::styled(
            format!(" {:>3.0} km/h", g.speed),
            Style::default()
                .fg(speed_color(frac))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("   NITRO ", lbl));
        spans.extend(gauge(g.nitro, 6, Color::Rgb(110, 225, 255)));
        if g.nitro_on && blink(g.time, 4.0) {
            spans.push(Span::styled(
                " »»",
                Style::default()
                    .fg(Color::Rgb(140, 235, 255))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if g.combo > 1 {
            spans.push(Span::styled(
                format!("   COMBO x{}", g.combo),
                Style::default()
                    .fg(Color::Rgb(255, 120, 220))
                    .add_modifier(Modifier::BOLD),
            ));
        }
        spans.push(Span::styled("   DIST ", lbl));
        spans.push(Span::styled(
            format!("{:.2} km", g.distance / 1000.0),
            Style::default().fg(Color::Rgb(190, 190, 200)),
        ));
        Paragraph::new(Line::from(spans)).render(inner, buf);
    }
}

struct Footer<'a>(&'a Game);
impl Widget for Footer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let g = self.0;
        let name = if g.state == State::Menu {
            g.sel.name()
        } else {
            g.diff.name()
        };
        let left = " ←→ steer · ↑ gas · ↓ brake · SPACE nitro · P pause · M snd · Q quit ";
        let right = format!(
            " {} · SND {} · v1.0 ",
            name,
            if g.sound { "ON" } else { "OFF" }
        );
        let pad = area
            .width
            .saturating_sub(left.chars().count() as u16 + right.chars().count() as u16)
            as usize;
        let dim = Style::default().fg(Color::Rgb(105, 105, 120));
        let line = Line::from(vec![
            Span::styled(left, dim),
            Span::raw(" ".repeat(pad)),
            Span::styled(right, Style::default().fg(Color::Rgb(170, 150, 90))),
        ]);
        Paragraph::new(line)
            .style(Style::default().bg(Color::Rgb(12, 12, 16)))
            .render(area, buf);
    }
}

// ── overlays ──────────────────────────────────────────────────────────────
fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w.min(area.width),
        h.min(area.height),
    )
}
fn checker_line(n: usize) -> Line<'static> {
    let mut spans = Vec::with_capacity(n);
    for i in 0..n {
        let st = if i % 2 == 0 {
            Style::default().fg(Color::Rgb(235, 235, 240))
        } else {
            Style::default().fg(Color::Rgb(70, 70, 78))
        };
        spans.push(Span::styled("▚", st));
    }
    Line::from(spans)
}
fn panel_block(border: Color) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(Color::Rgb(10, 10, 14)))
}

fn menu_overlay(f: &mut Frame, g: &Game) {
    let area = f.area();
    let rect = centered_rect(62, 22, area);
    f.render_widget(Clear, rect);
    let mut lines: Vec<Line> = Vec::new();
    let inner_w = rect.width.saturating_sub(4) as usize;
    lines.push(checker_line(inner_w));
    let logo_style = Style::default()
        .fg(Color::Rgb(255, 204, 60))
        .add_modifier(Modifier::BOLD);
    for row in big_text("RAT RACER") {
        lines.push(Line::from(Span::styled(row, logo_style)));
    }
    lines.push(checker_line(inner_w));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "rémy the rat has a lead foot. keep up.",
        Style::default()
            .fg(Color::Rgb(150, 150, 165))
            .add_modifier(Modifier::ITALIC),
    )));
    let frames = ["<:3)~~~~", "<:3)~~~", "<:3)~~~~~~", "<:3) ~~~"];
    let rat = frames[(g.time * 7.0) as usize % 4];
    lines.push(Line::from(vec![
        Span::styled(
            rat,
            Style::default()
                .fg(Color::Rgb(240, 240, 245))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "   « the driver »",
            Style::default().fg(Color::Rgb(110, 110, 125)),
        ),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "— INTENSITY —",
        Style::default().fg(Color::Rgb(110, 110, 125)),
    )));
    let mut diff_spans: Vec<Span> = Vec::new();
    for (i, d) in Difficulty::ALL.iter().enumerate() {
        if *d == g.sel {
            diff_spans.push(Span::styled(
                format!("▶ [{}] {} ", i + 1, d.name()),
                Style::default()
                    .fg(Color::Rgb(255, 214, 80))
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            diff_spans.push(Span::styled(
                format!("  [{}] {} ", i + 1, d.name()),
                Style::default().fg(Color::Rgb(110, 110, 125)),
            ));
        }
        diff_spans.push(Span::raw("  "));
    }
    lines.push(Line::from(diff_spans));
    lines.push(Line::from(Span::styled(
        g.sel.blurb(),
        Style::default().fg(Color::Rgb(110, 200, 220)),
    )));
    lines.push(Line::from(""));
    let key = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default()
                .fg(Color::Rgb(240, 240, 245))
                .add_modifier(Modifier::BOLD),
        )
    };
    let lab = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default().fg(Color::Rgb(110, 110, 125)),
        )
    };
    lines.push(Line::from(vec![
        key("←→"),
        lab(" steer   "),
        key("↑"),
        lab(" gas   "),
        key("↓"),
        lab(" brake   "),
        key("SPACE"),
        lab(" nitro   "),
        key("P"),
        lab(" pause"),
    ]));
    lines.push(Line::from(""));
    let prompt_style = if blink(g.time, 1.4) {
        Style::default()
            .fg(Color::Rgb(130, 255, 150))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Rgb(60, 110, 70))
    };
    lines.push(Line::from(Span::styled(
        "▶ PRESS ENTER TO IGNITE ◀",
        prompt_style,
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "best · chill {:06} · rush {:06} · mayhem {:06}",
            g.hi[0], g.hi[1], g.hi[2]
        ),
        Style::default().fg(Color::Rgb(160, 140, 80)),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "M sound: {}      Q quit",
            if g.sound { "ON" } else { "OFF" }
        ),
        Style::default().fg(Color::Rgb(90, 90, 105)),
    )));
    let para = Paragraph::new(lines).alignment(Alignment::Center);
    f.render_widget(panel_block(Color::Rgb(170, 130, 40)), rect);
    f.render_widget(para, centered_rect(rect.width - 2, rect.height - 2, rect));
}

fn gameover_overlay(f: &mut Frame, g: &Game) {
    let area = f.area();
    let rect = centered_rect(54, 19, area);
    f.render_widget(Clear, rect);
    let mut lines: Vec<Line> = Vec::new();
    let inner_w = rect.width.saturating_sub(4) as usize;
    lines.push(checker_line(inner_w));
    let red = Style::default()
        .fg(Color::Rgb(255, 80, 65))
        .add_modifier(Modifier::BOLD);
    for row in big_text("WRECKED!") {
        lines.push(Line::from(Span::styled(row, red)));
    }
    lines.push(checker_line(inner_w));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        QUIPS[g.quip],
        Style::default()
            .fg(Color::Rgb(150, 150, 165))
            .add_modifier(Modifier::ITALIC),
    )));
    lines.push(Line::from(""));
    let lbl = Style::default().fg(Color::Rgb(118, 118, 134));
    let val = Style::default()
        .fg(Color::Rgb(245, 245, 250))
        .add_modifier(Modifier::BOLD);
    let mut score_line = vec![
        Span::styled("SCORE      ", lbl),
        Span::styled(format!("{:06}", g.score as u64), val),
    ];
    if g.new_best && blink(g.time, 3.0) {
        score_line.push(Span::styled(
            "  ★ NEW BEST!",
            Style::default()
                .fg(Color::Rgb(255, 214, 80))
                .add_modifier(Modifier::BOLD),
        ));
    }
    lines.push(Line::from(score_line));
    lines.push(Line::from(vec![
        Span::styled("DISTANCE   ", lbl),
        Span::styled(format!("{:.2} km", g.distance / 1000.0), val),
    ]));
    lines.push(Line::from(vec![
        Span::styled("TOP SPEED  ", lbl),
        Span::styled(format!("{:.0} km/h", g.top_speed), val),
    ]));
    lines.push(Line::from(vec![
        Span::styled("NEAR MISS  ", lbl),
        Span::styled(format!("{}", g.near_misses), val),
    ]));
    lines.push(Line::from(vec![
        Span::styled("BEST COMBO ", lbl),
        Span::styled(format!("x{}", g.best_combo), val),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "ENTER retry · ESC menu",
        Style::default().fg(Color::Rgb(110, 110, 125)),
    )));
    f.render_widget(panel_block(Color::Rgb(160, 60, 50)), rect);
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        centered_rect(rect.width - 2, rect.height - 2, rect),
    );
}

fn pause_overlay(f: &mut Frame, _g: &Game) {
    let area = f.area();
    let rect = centered_rect(42, 9, area);
    f.render_widget(Clear, rect);
    f.render_widget(panel_block(Color::Rgb(120, 120, 140)), rect);
    let inner = centered_rect(rect.width - 2, rect.height - 2, rect);
    let buf = f.buffer_mut();
    let mut p = Painter::new(buf, (inner.x as i32, inner.y as i32));
    draw_big(
        &mut p,
        inner.width as i32 / 2,
        1,
        &big_text("PAUSED"),
        Style::default()
            .fg(Color::Rgb(255, 214, 80))
            .add_modifier(Modifier::BOLD),
        Some(Style::default().fg(Color::Rgb(40, 35, 10))),
    );
    p.put_str(
        inner.width as i32 / 2 - 8,
        5,
        "P resume · Q quit",
        Style::default().fg(Color::Rgb(110, 110, 125)),
    );
}

fn countdown_overlay(f: &mut Frame, g: &Game, world: Rect) {
    let n = (g.countdown.ceil() as i32).clamp(1, 3);
    let buf = f.buffer_mut();
    let mut p = Painter::new(buf, (world.x as i32, world.y as i32));
    draw_big(
        &mut p,
        world.width as i32 / 2,
        world.height as i32 / 2 - 4,
        &big_text(&n.to_string()),
        Style::default()
            .fg(Color::Rgb(255, 214, 80))
            .add_modifier(Modifier::BOLD),
        Some(Style::default().fg(Color::Rgb(70, 30, 10))),
    );
    let hint = "get ready — steer with ← →";
    p.put_str(
        world.width as i32 / 2 - hint.chars().count() as i32 / 2,
        world.height as i32 / 2 + 1,
        hint,
        Style::default().fg(Color::Rgb(120, 120, 135)),
    );
}

fn go_overlay(f: &mut Frame, g: &Game, world: Rect) {
    let buf = f.buffer_mut();
    let mut p = Painter::new(buf, (world.x as i32, world.y as i32));
    let mut st = Style::default()
        .fg(Color::Rgb(130, 255, 150))
        .add_modifier(Modifier::BOLD);
    if g.run_t > 0.5 {
        st = st.add_modifier(Modifier::DIM);
    }
    draw_big(
        &mut p,
        world.width as i32 / 2,
        world.height as i32 / 2 - 3,
        &big_text("GO!"),
        st,
        Some(Style::default().fg(Color::Rgb(15, 50, 25))),
    );
}

// ── frame composition ─────────────────────────────────────────────────────
fn shake(g: &Game) -> (i32, i32) {
    if g.shake > 0.01 {
        (
            ((g.time * 87.0).sin() * g.shake * 2.4).round() as i32,
            ((g.time * 71.0).cos() * g.shake * 1.6).round() as i32,
        )
    } else {
        (0, 0)
    }
}

fn draw(f: &mut Frame, g: &Game) {
    let area = f.area();
    if area.width < 46 || area.height < 17 {
        let msg = Paragraph::new("terminal too small — need at least 46×17")
            .style(Style::default().fg(Color::Rgb(255, 170, 90)));
        f.render_widget(msg, centered_rect(40, 1, area));
        return;
    }
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);
    let world = chunks[1];
    let (sx, sy) = shake(g);
    {
        let mut p = Painter::new(f.buffer_mut(), (world.x as i32 + sx, world.y as i32 + sy));
        render_world(&mut p, g, world);
    }
    f.render_widget(Hud(g), chunks[0]);
    f.render_widget(Footer(g), chunks[2]);
    match g.state {
        State::Menu => menu_overlay(f, g),
        State::GameOver => gameover_overlay(f, g),
        State::Paused => pause_overlay(f, g),
        State::Countdown => countdown_overlay(f, g, world),
        State::Running if g.run_t < 0.9 => go_overlay(f, g, world),
        _ => {}
    }
}

// ── high-score persistence ────────────────────────────────────────────────
fn hi_path() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|p| p.join(".rat_racer_hi"))
        .unwrap_or_else(|| PathBuf::from(".rat_racer_hi"))
}
fn load_hi() -> [u64; 3] {
    let mut hi = [0u64; 3];
    if let Ok(s) = std::fs::read_to_string(hi_path()) {
        for (i, line) in s.lines().take(3).enumerate() {
            hi[i] = line.trim().parse().unwrap_or(0);
        }
    }
    hi
}
fn save_hi(hi: &[u64; 3]) {
    let s = format!("{}\n{}\n{}\n", hi[0], hi[1], hi[2]);
    let _ = std::fs::write(hi_path(), s);
}

// ── main loop ─────────────────────────────────────────────────────────────
fn restore() -> io::Result<()> {
    terminal::disable_raw_mode()?;
    execute!(io::stdout(), Show, LeaveAlternateScreen)?;
    Ok(())
}

fn run() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut game = Game::new(load_hi());
    let mut input = Input::default();
    let mut full = term.get_frame().area();
    let mut last = Instant::now();

    loop {
        let dt = last.elapsed().as_secs_f64().min(0.1);
        last = Instant::now();

        while event::poll(Duration::ZERO)? {
            if let Event::Key(k) = event::read()? {
                if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                    return Ok(());
                }
                let now = game.time;
                match k.kind {
                    KeyEventKind::Release => input.release(k.code),
                    _ => {
                        let press_only = k.kind == KeyEventKind::Press;
                        match k.code {
                            KeyCode::Char('q') => return Ok(()),
                            KeyCode::Esc => match game.state {
                                State::Menu => return Ok(()),
                                State::GameOver => game.to_menu(),
                                State::Running => game.state = State::Paused,
                                State::Paused => game.state = State::Running,
                                _ => {}
                            },
                            KeyCode::Char('m') => {
                                if press_only {
                                    game.sound = !game.sound;
                                }
                            }
                            KeyCode::Char('p') => {
                                if press_only {
                                    match game.state {
                                        State::Running => game.state = State::Paused,
                                        State::Paused => game.state = State::Running,
                                        _ => {}
                                    }
                                }
                            }
                            KeyCode::Enter => match game.state {
                                State::Menu => {
                                    let d = game.sel;
                                    game.start_run(d, world_rect(full));
                                }
                                State::GameOver => {
                                    let d = game.diff;
                                    game.start_run(d, world_rect(full));
                                }
                                _ => {}
                            },
                            KeyCode::Char('1') => game.sel = Difficulty::Chill,
                            KeyCode::Char('2') => game.sel = Difficulty::Rush,
                            KeyCode::Char('3') => game.sel = Difficulty::Mayhem,
                            KeyCode::Left => {
                                if game.state == State::Menu && press_only {
                                    let i = game.sel.index();
                                    game.sel = Difficulty::ALL[(i + 2) % 3];
                                }
                                input.press(k.code, now);
                            }
                            KeyCode::Right => {
                                if game.state == State::Menu && press_only {
                                    let i = game.sel.index();
                                    game.sel = Difficulty::ALL[(i + 1) % 3];
                                }
                                input.press(k.code, now);
                            }
                            c => input.press(c, now),
                        }
                    }
                }
            }
        }

        let world = world_rect(full);
        game.update(dt, world, &input);
        term.draw(|f| {
            full = f.area();
            draw(f, &game);
        })?;
        thread::sleep(Duration::from_millis(8));
    }
}

fn world_rect(full: Rect) -> Rect {
    Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(full)[1]
}

fn main() -> io::Result<()> {
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        default_panic(info);
    }));
    let result = run();
    let _ = restore();
    result
}
