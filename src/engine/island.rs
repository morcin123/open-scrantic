//! The island and its ambient animation (seg3 of the original): background composition,
//! placement, shoreline waves, ocean whitecaps, clouds, raft and holiday decorations.

use std::rc::Rc;

use super::gfx::{Canvas, Clip, WIDTH};
use super::rng::Rng;
use crate::assets::Result;
use crate::assets::resource::{Archive, Bmp};

/// Background mode (0x30B4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Night,
    NoIsland,
}

#[derive(Clone, Copy, Default)]
struct Slot {
    x: i32,
    y: i32,
    frame: usize,
    kind: usize,
    interval: i32,
    counter: i32,
}

impl Slot {
    fn new(x: i32, y: i32, kind: usize, interval: i32, counter: i32) -> Slot {
        Slot {
            x,
            y,
            frame: 0,
            kind,
            interval,
            counter,
        }
    }

    fn empty(&self) -> bool {
        self.x == 0 && self.y == 0
    }

    /// Post-decrements the counter; true when it was already zero.
    fn tick(&mut self) -> bool {
        let expired = self.counter == 0;
        self.counter -= 1;
        expired
    }
}

pub struct Options {
    pub background: bool,
    pub clouds: bool,
    pub waves: bool,
}

/// Shoreline wave spots: (x, y, w, h, first sprite, phase counter index), relative to the origin.
const HIGH_WAVES: [(i32, i32, i32, i32, usize, usize); 3] = [
    (0x10F, 0x132, 0x48, 0x1D, 3, 0),
    (0x16D, 0x13F, 0x9C, 0x19, 6, 1),
    (0x207, 0x12F, 0x41, 0x20, 9, 2),
];
const LOW_WAVES: [(i32, i32, i32, i32, usize, usize); 4] = [
    (0xEA, 0x143, 0x78, 0x30, 30, 0),
    (0x170, 0x164, 0xAB, 0x1C, 33, 1),
    (0x22F, 0x143, 0x51, 0x30, 36, 2),
    (0x82, 0x154, 0x61, 0x1D, 39, 0),
];
const TRUNK: (i32, i32, i32, i32) = (0x1BB, 0x94, 0x16, 0x91);
const LEAVES: (i32, i32, i32, i32) = (0x16E, 0x7A, 0x94, 0x45);
const SHADOW: (i32, i32, i32, i32) = (0x18D, 0x117, 0x68, 0x26);
const ISLAND: (i32, i32, i32, i32) = (0x121, 0x117, 0x117, 0x33);
const LOW_SHORE: (i32, i32, i32, i32) = (0xFA, 0x12F, 0x17C, 0x45);
const ROCK: (i32, i32, i32, i32) = (0x97, 0x148, 0x39, 0x1E);

/// seg3:1a88 – 1 if rectangles intersect, +1 if the first lies inside the second.
/// Not in the original: a picture of the island alone, `w` x `h` palette indices, `None` where see-through.
pub struct IslandPicture {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Option<u8>>,
}

/// Not in the original: the row where the sky meets the sea: the first one that differs from the row above it in
/// more than 5/8 of its pixels (the sky is flat, the sea dithered row by row).
fn horizon(c: &Canvas) -> i32 {
    let w = WIDTH as usize;
    (32..400)
        .find(|&y| {
            let (row, above) = (&c.px[y * w..][..w], &c.px[(y - 1) * w..][..w]);
            row.iter().zip(above).filter(|(a, b)| a != b).count() * 8 > w * 5
        })
        .unwrap_or(0) as i32
}

fn overlap(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> i32 {
    let inside = a.0 >= b.0 && a.0 + a.2 < b.0 + b.2 && a.1 >= b.1 && a.1 + a.3 < b.1 + b.3;
    let hit = a.0 + a.2 >= b.0 && b.0 + b.2 > a.0 && a.1 + a.3 >= b.1 && b.1 + b.3 > a.1;
    i32::from(inside) + i32::from(hit)
}

/// seg3:14bf – random position inside a rectangle (minus the sprite size).
fn rand_pos(rng: &mut Rng, size: Option<(i32, i32)>, x0: i32, y0: i32, mut w: i32, mut h: i32) -> (i32, i32) {
    if let Some((sw, sh)) = size {
        w -= sw;
        h -= sh;
    }
    if w < 0 || h < 0 {
        return (0, 0);
    }
    let x = x0 + if w != 0 { rng.below(w as u16) as i32 } else { 0 };
    let y = y0 + if h != 0 { rng.below(h as u16) as i32 } else { 0 };
    (x, y)
}

pub struct Island {
    /// Not in the original: only the ocean, sky and clouds (with whitecaps), no island, for the other monitors.
    pub sea_only: bool,
    /// The background picture of the current session (OCEAN0n.SCR or NIGHT.SCR); a sea-only island uses the one it
    /// is given here.
    pub picture: String,
    /// Not in the original: the row of the sky's edge in the session picture (0 without one), where the far island
    /// of another monitor sits.
    pub horizon: i32,
    pub cfg: i32,
    pub tide: i32,
    pub no_raft: bool,
    pub freeze_waves: bool,
    pub mode: Mode,
    pub holiday_ok: bool,
    pub phase: i32,
    pub holiday: i32,
    pub x: i32,
    pub y: i32,
    pub wave_timer: u32,
    pub cap_timer: u32,
    waves: [i32; 3],
    wind: i32,
    raft_stage: i32,
    caps: Vec<Slot>,
    cap_quota: [i32; 4],
    background: Rc<Bmp>,
    raft: Rc<Bmp>,
    holiday_bmp: Rc<Bmp>,
    cap_seqs: [Vec<u8>; 3],
}

impl Island {
    pub fn new(archive: &Archive, cap_seqs: [Vec<u8>; 3]) -> Result<Self> {
        Ok(Self {
            sea_only: false,
            picture: String::new(),
            horizon: 0,
            cfg: -1,
            tide: -1,
            no_raft: false,
            freeze_waves: false,
            mode: Mode::Normal,
            holiday_ok: true,
            phase: 0,
            holiday: 0,
            x: 0,
            y: 0,
            wave_timer: 0,
            cap_timer: 0,
            waves: [2; 3],
            wind: 1,
            raft_stage: 0,
            caps: Vec::new(),
            cap_quota: [0; 4],
            background: Rc::new(archive.bmp("BACKGRND.BMP")?),
            raft: Rc::new(archive.bmp("MRAFT.BMP")?),
            holiday_bmp: Rc::new(archive.bmp("HOLIDAY.BMP")?),
            cap_seqs,
        })
    }

    /// seg3:1c41(-1, -1)
    pub fn reset_config(&mut self) {
        self.cfg = -1;
        self.tide = -1;
        self.freeze_waves = false;
        self.mode = Mode::Normal;
        self.no_raft = false;
        self.holiday_ok = true;
    }

    /// seg3:1c41 – accumulate the island requirements of a planned scene.
    pub fn apply_flags(&mut self, flags: u32) {
        let (lo, hi) = (flags & 0xFFFF, flags >> 16);
        if lo & 0x200 != 0 {
            self.tide = 0;
        }
        if lo & 0x4000 != 0 {
            self.no_raft = true;
        }
        if lo & 0x10 != 0 {
            self.holiday_ok = false;
        }
        if lo & 0x1000 != 0 {
            self.freeze_waves = true;
        }
        if lo & 0x800 != 0 {
            self.mode = Mode::Night;
        }
        if lo & 0x2000 != 0 {
            self.mode = Mode::NoIsland;
        }
        if self.cfg == -1 && hi & 0x3F != 0 {
            self.cfg = 0;
        }
        if hi & 0x20 != 0 {
            self.cfg = 1;
        } else if hi & 0x10 != 0 {
            self.cfg = 2;
        }
        for (bit, set) in [(1, 4), (2, 8), (4, 0x10), (8, 0x20)] {
            if hi & bit != 0 {
                self.cfg |= set;
            }
        }
    }

    /// seg4:0edb – can a scene with these flags share the island layout chosen so far?
    pub fn compatible(&self, flags: u32) -> bool {
        let (hi, c) = (flags >> 16, self.cfg);
        if c == -1 {
            return true;
        }
        let has = |b: u32| hi & b != 0;
        let conflicts = [
            (has(0x20), c & 0x2A),
            (has(0x10), c & 0x15),
            (has(1) && !has(2), c & 0x2A),
            (has(1) && !has(4), c & 0x30),
            (has(2) && !has(1), c & 0x15),
            (has(2) && !has(8), c & 0x30),
            (has(4) && !has(8), c & 0x2A),
            (has(4) && !has(1), c & 0x0C),
            (has(8) && !has(4), c & 0x15),
            (has(8) && !has(2), c & 0x0C),
        ];
        !conflicts.iter().any(|&(cond, bits)| cond && bits != 0)
    }

    /// seg3:00fc – drawing offset applied to TTM coordinates.
    pub fn ttm_offset(&self, shifted: bool) -> (i32, i32) {
        if self.mode == Mode::NoIsland {
            (0, 0)
        } else {
            (self.x + 1 + if shifted { 0x110 } else { 0 }, self.y)
        }
    }

    fn sprite(&self, c: &mut Canvas, index: usize, x: i32, y: i32, flags: u8, clip: Clip) {
        if let Some(s) = self.background.sprites.get(index) {
            c.sprite(s, self.x + x, self.y + y, flags, clip);
        }
    }

    fn rect(&self, r: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
        (self.x + r.0, self.y + r.1, r.2, r.3)
    }

    /// seg3:1b00 – choose the island position honoring the accumulated constraints.
    fn place(&mut self, rng: &mut Rng) {
        if self.cfg == -1 {
            self.cfg = 0;
        }
        let c = self.cfg;
        let (x, y) = if c & 2 != 0 {
            (0x121, 0x12B)
        } else if c & 1 != 0 {
            (0x11, 0x135)
        } else {
            let (mut x0, mut y0, mut w, mut h) = if self.tide == 1 {
                (0xA8, 0xFF, 0x6F, 0x3B)
            } else {
                (0x43, 0xFF, 0xE2, 0x81)
            };
            if c != 0 {
                if matches!(c, 0x10 | 0x20 | 0x30) {
                    y0 += h / 2;
                }
                if !((c & 4 != 0 && c & 0x10 != 0) || (c & 8 != 0 && c & 0x20 != 0)) {
                    h /= 2;
                }
                if matches!(c, 8 | 0x20 | 0x28) {
                    x0 += w / 2;
                }
                if !((c & 4 != 0 && c & 8 != 0) || (c & 0x10 != 0 && c & 0x20 != 0)) {
                    w /= 2;
                }
            }
            rand_pos(rng, None, x0, y0, w, h)
        };
        self.x = x - 0x121;
        self.y = y - 0x117;
    }

    /// seg3:0136 – build the session background (into `bg`) and reset the ambient animation.
    pub fn init(&mut self, archive: &Archive, bg: &mut Canvas, rng: &mut Rng, opts: &Options, max_day: i32) {
        self.horizon = 0;
        if !opts.background || self.mode == Mode::NoIsland {
            bg.fill(5);
        } else {
            let name = if self.sea_only && !self.picture.is_empty() {
                self.picture.clone()
            } else if self.mode == Mode::Normal && self.phase < 12 {
                format!("OCEAN0{}.SCR", rng.below(3))
            } else {
                "NIGHT.SCR".to_string()
            };
            self.picture.clone_from(&name);
            match archive.scr(&name) {
                Ok(pic) => {
                    bg.load_picture(&pic);
                    self.horizon = horizon(bg);
                }
                Err(e) => eprintln!("warning: {e}"),
            }
        }
        if self.tide == -1 {
            self.tide = (rng.next_u16() & 1) as i32;
        }
        self.place(rng);
        if self.mode != Mode::NoIsland && !self.sea_only {
            self.raft_stage = (max_day - 1).clamp(1, 5);
            self.draw_raft(bg);
            self.sprite(bg, 0, ISLAND.0, ISLAND.1, 0, Clip::FULL);
            if self.tide == 1 {
                self.sprite(bg, 1, LOW_SHORE.0, LOW_SHORE.1, 0, Clip::FULL);
                self.sprite(bg, 2, ROCK.0, ROCK.1, 0, Clip::FULL);
            }
            self.waves = [2; 3];
            self.wave_timer = 0;
            self.update_waves();
            self.draw_waves(bg);
            self.sprite(bg, 14, SHADOW.0, SHADOW.1, 0, Clip::FULL);
            self.sprite(bg, 13, TRUNK.0, TRUNK.1, 0, Clip::FULL);
            self.sprite(bg, 12, LEAVES.0, LEAVES.1, 0, Clip::FULL);
        }
        let caps = if opts.waves && !self.freeze_waves { 30 } else { 0 };
        self.caps = vec![Slot::default(); caps];
        self.cap_quota = [30, 4, 2, 1];
        let mut work = bg.clone();
        self.init_clouds(bg, &mut work, rng, if opts.clouds { 15 } else { 0 });
    }

    /// Per-step ambient animation (seg3:040e(0)), drawn into the work buffer.
    pub fn step(&mut self, work: &mut Canvas, rng: &mut Rng) {
        if self.mode == Mode::NoIsland {
            return;
        }
        self.whitecaps(work, rng);
        if !self.freeze_waves && !self.sea_only {
            self.update_waves();
            self.draw_waves(work);
            self.draw_raft(work);
        }
    }

    /// seg3:0e74(0) – stagger the three wave phase counters every 4 ticks.
    fn update_waves(&mut self) {
        if self.wave_timer != 0 {
            return;
        }
        self.wave_timer = 4;
        let w = &mut self.waves;
        if w[0] == w[2] {
            w[0] += 1;
        } else if w[1] != w[0] {
            w[1] += 1;
        } else {
            w[2] += 1;
        }
        for v in w.iter_mut() {
            if *v > 2 {
                *v = 0;
            }
        }
    }

    fn wave_spots(&self) -> &'static [(i32, i32, i32, i32, usize, usize)] {
        if self.tide == 1 { &LOW_WAVES } else { &HIGH_WAVES }
    }

    fn wave_rect(&self, w: (i32, i32, i32, i32, usize, usize)) -> (i32, i32, i32, i32) {
        self.rect((w.0, w.1, w.2, w.3))
    }

    fn draw_wave(&self, c: &mut Canvas, spot: (i32, i32, i32, i32, usize, usize), clip: Clip) {
        let (x, y, _, _, first, counter) = spot;
        self.sprite(c, first + self.waves[counter] as usize, x, y, 0, clip);
    }

    fn draw_waves(&self, c: &mut Canvas) {
        for &spot in self.wave_spots() {
            self.draw_wave(c, spot, Clip::FULL);
        }
    }

    /// seg3:1d69 – the raft grows with the story day.
    fn draw_raft(&self, c: &mut Canvas) {
        if self.no_raft || self.raft_stage == 0 {
            return;
        }
        let (x, y) = if self.tide == 0 { (0x201, 0x10A) } else { (0x212, 0x119) };
        if let Some(s) = self.raft.sprites.get(self.raft_stage as usize - 1) {
            c.sprite(s, self.x + x, self.y + y, 0, Clip::FULL);
        }
    }

    /// Not in the original: the island with its palm as one picture, to draw it small and far away on another
    /// monitor. Returns (width, height, palette index per pixel or None where it is see-through).
    pub fn far_island(&self) -> IslandPicture {
        // Drawn on two blank canvases of different colours: a pixel is part of the island where both agree.
        // At the island origin (0, 0), where all of it is on the canvas.
        let (mut a, mut b) = (Canvas::default(), Canvas::default());
        a.fill(0);
        b.fill(1);
        let parts = [(0, ISLAND), (14, SHADOW), (13, TRUNK), (12, LEAVES)];
        for c in [&mut a, &mut b] {
            for (index, r) in parts {
                if let Some(s) = self.background.sprites.get(index) {
                    c.sprite(s, r.0, r.1, 0, Clip::FULL);
                }
            }
        }
        let x0 = parts.iter().map(|p| p.1.0).min().unwrap_or(0).max(0);
        let y0 = parts.iter().map(|p| p.1.1).min().unwrap_or(0).max(0);
        let x1 = parts.iter().map(|p| p.1.0 + p.1.2).max().unwrap_or(0).min(WIDTH);
        let y1 = parts
            .iter()
            .map(|p| p.1.1 + p.1.3)
            .max()
            .unwrap_or(0)
            .min(super::gfx::HEIGHT);
        let (w, h) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
        let mut px = Vec::with_capacity(w * h);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * WIDTH + x) as usize;
                px.push((a.px[i] == b.px[i]).then_some(a.px[i]));
            }
        }
        IslandPicture { w, h, px }
    }

    /// seg3:1e50 – holiday decoration, drawn on top every step.
    pub fn draw_holiday(&self, c: &mut Canvas) {
        if self.holiday == 0 || !self.holiday_ok || self.mode == Mode::NoIsland {
            return;
        }
        let (index, x, y) = match self.holiday {
            1 => (0, 0x19B, 0x12A),
            2 => (2, 0x195, 0x10B),
            3 => (3, 0x16A, 0x9B),
            _ => (1, 0x14E, 0x11E),
        };
        if let Some(s) = self.holiday_bmp.sprites.get(index) {
            c.sprite(s, self.x + x, self.y + y, 0, Clip::FULL);
        }
    }

    /// seg3:1551 with 0x2E7A=0 – after Johnny walks by, put the trunk back in front of him
    /// when he is behind the palm tree.
    pub fn trunk_overlay(&self, c: &mut Canvas, r: (i32, i32, i32, i32)) {
        if r.1 + r.3 < self.y + 0x125 && overlap(r, self.rect(TRUNK)) != 0 {
            self.sprite(c, 13, TRUNK.0, TRUNK.1, 0, Clip::rect(r.0, r.1, r.2, r.3));
        }
    }

    /// seg3:1551 with 0x2E7A=1 – redraw the island parts covering a rectangle.
    fn redraw_island_over(&self, c: &mut Canvas, r: (i32, i32, i32, i32)) {
        if self.sea_only {
            return;
        }
        let clip = Clip::rect(r.0, r.1, r.2, r.3);
        let bottom = (r.0, r.1 + r.3, r.2, 1);
        if overlap(r, (0, self.y + 0xD6, WIDTH, 0x10A)) != 0 {
            let part = |c: &mut Canvas, idx: usize, rect: (i32, i32, i32, i32)| {
                if overlap(r, self.rect(rect)) != 0 {
                    self.sprite(c, idx, rect.0, rect.1, 0, clip);
                }
            };
            if self.tide == 1 {
                part(c, 2, ROCK);
                let d = LOW_WAVES[3];
                if overlap(bottom, self.wave_rect(d)) != 0 {
                    self.draw_wave(c, d, clip);
                }
                part(c, 0, ISLAND);
                part(c, 1, LOW_SHORE);
                for &w in &LOW_WAVES[..3] {
                    if overlap(bottom, self.wave_rect(w)) != 0 {
                        self.draw_wave(c, w, clip);
                    }
                }
            } else {
                part(c, 0, ISLAND);
                for &w in &HIGH_WAVES {
                    if overlap(bottom, self.wave_rect(w)) != 0 {
                        self.draw_wave(c, w, clip);
                    }
                }
            }
        }
        for (idx, rect) in [(13, TRUNK), (12, LEAVES), (14, SHADOW)] {
            if overlap(r, self.rect(rect)) != 0 {
                self.sprite(c, idx, rect.0, rect.1, 0, clip);
            }
        }
    }

    /// seg3:0a38 + one pass of seg3:0b67 – clouds are placed once and baked into the background.
    fn init_clouds(&mut self, bg: &mut Canvas, work: &mut Canvas, rng: &mut Rng, count: usize) {
        let mut quota = [8, 4, 2]; // small, medium, big
        self.wind = if rng.pick(&[50]) == 1 { -1 } else { 1 };
        let size = |bmp: &Bmp, kind: usize| bmp.sprites.get(15 + kind).map(|s| (s.w, s.h)).unwrap_or((0, 0));
        let mut clouds = vec![Slot::default(); count];
        for c in clouds.iter_mut() {
            if rng.pick(&[30]) != 1 {
                continue;
            }
            // Bigger clouds fall back to smaller ones when their quota is exhausted.
            let mut kind = match rng.pick(&[5, 10, 15]) {
                1 => Some(2),
                2 => Some(1),
                3 => Some(0),
                _ => None,
            };
            while let Some(k) = kind {
                if quota[k] > 0 {
                    break;
                }
                kind = k.checked_sub(1);
            }
            let Some(k) = kind else { continue };
            quota[k] -= 1;
            let (x, y) = rand_pos(rng, Some(size(&self.background, k)), 0, 1, WIDTH, 0x8B);
            let interval = rng.below(5) as i32 + 6;
            *c = Slot::new(x, y, k, interval, rng.below(interval as u16) as i32);
        }
        for c in clouds.iter_mut().filter(|c| c.empty()) {
            let k = match rng.pick(&[20, 30, 50]) {
                1 => 2,
                2 => 1,
                3 => 0,
                _ => continue,
            };
            if quota[k] == 0 {
                continue;
            }
            quota[k] -= 1;
            let x0 = if self.wind == 1 { -1000 } else { WIDTH };
            let (x, y) = rand_pos(rng, Some(size(&self.background, k)), x0, 1, 1000, 0x8B);
            *c = Slot::new(x, y, k, rng.below(5) as i32 + 6, 0);
        }
        for c in clouds.iter_mut().filter(|c| !c.empty()) {
            if c.tick() {
                c.counter = c.interval;
                c.x += self.wind;
            }
        }
        let flags = if self.wind > 0 { super::gfx::FLIP_H } else { 0 };
        for c in clouds.iter().filter(|c| !c.empty()) {
            let Some(s) = self.background.sprites.get(15 + c.kind) else {
                continue;
            };
            if c.x + s.w > 0 && c.x < WIDTH {
                work.sprite(s, c.x, c.y, flags, Clip::FULL);
                self.redraw_island_over(work, (c.x, c.y, s.w, s.h));
                bg.copy_rect_from(work, c.x, c.y, s.w, s.h);
            }
        }
    }

    /// seg3:04a2 – ocean whitecaps and glints drifting with the wind.
    fn whitecaps(&mut self, work: &mut Canvas, rng: &mut Rng) {
        let moving = self.cap_timer == 0;
        for i in 0..self.caps.len() {
            let mut c = self.caps[i];
            if c.empty() && rng.chance(35) {
                match rng.pick(&[35, 5, 5, 5]) {
                    1 if self.cap_quota[0] > 0 => {
                        let interval = rng.below(30) as i32 + 10;
                        let (x, y) = rand_pos(rng, None, 0, 0xD6, WIDTH, 0x23);
                        c = Slot::new(x, y, 0, interval, interval);
                        let point = (x, y, 1, 1);
                        if overlap(point, self.rect(TRUNK)) == 0 && overlap(point, self.rect(LEAVES)) == 0 {
                            self.cap_quota[0] -= 1;
                        } else {
                            c = Slot::default();
                        }
                    }
                    n @ 2..=4 if self.cap_quota[n - 1] > 0 => {
                        let kind = n - 1;
                        let interval = rng.pick(&[30, 50]) as i32;
                        let (x0, y0, w, h) = [
                            (-10, 0xF9, 0x28A, 0x28),
                            (-40, 0x121, 0x2A8, 0x5D),
                            (-70, 0x17E, 0x2C6, 0x8E),
                        ][kind - 1];
                        let (x, y) = rand_pos(rng, None, x0, y0, w, h);
                        c = Slot::new(x, y, kind, interval, interval);
                        self.cap_quota[kind] -= 1;
                    }
                    _ => {}
                }
            }
            if c.empty() {
                self.caps[i] = c;
                continue;
            }
            let mut sprite = None;
            if c.kind == 0 {
                work.pixel(c.x, c.y, 15, Clip::FULL);
                if moving && c.tick() {
                    c = Slot::default();
                    self.cap_quota[0] += 1;
                }
            } else {
                if moving {
                    c.x += self.wind;
                    if c.tick() {
                        c.counter = c.interval;
                        c.frame += 1;
                    }
                }
                match self.cap_seqs[3 - c.kind].get(c.frame) {
                    Some(&s) => sprite = Some(s as usize),
                    None => {
                        self.cap_quota[c.kind] += 1;
                        c = Slot::default();
                    }
                }
            }
            self.caps[i] = c;
            let Some(index) = sprite else { continue };
            let Some(s) = self.background.sprites.get(index) else {
                continue;
            };
            let r = (c.x, c.y, s.w, s.h);
            let hidden = !self.sea_only
                && self
                    .wave_spots()
                    .iter()
                    .take(3)
                    .any(|w| overlap(r, self.wave_rect(*w)) == 2);
            if !hidden {
                work.sprite(s, c.x, c.y, 0, Clip::FULL);
                self.redraw_island_over(work, r);
            }
        }
        if moving {
            self.cap_timer = 10;
        }
    }
}
