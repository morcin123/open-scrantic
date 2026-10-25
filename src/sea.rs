//! Not in the original: the picture on one monitor and the open sea on the others (and in the bars beside the
//! picture), lined up so that the horizon runs on across all of them. The monitor farthest out gets the island far
//! away, near its outer edge. Like `present.rs`, this only changes what reaches the screen.

use crate::engine::gfx::{HEIGHT, WIDTH};
use crate::engine::island::IslandPicture;

const W: i64 = WIDTH as i64;
const H: i64 = HEIGHT as i64;

/// The far island is this many times smaller than the one in the picture.
const FAR_SHRINK: usize = 5;
/// It sits this many picture rows below the horizon, and this far (in parts of the monitor's width) from the outer
/// edge.
const FAR_DROP: i64 = 3;
const FAR_MARGIN: f64 = 0.14;
/// Share of the sea behind mixed into the far island (in 256ths): haze.
const FAR_HAZE: u32 = 72;
/// Below the picture the sea repeats its lowest rows, this many of them, without their whitecaps (which would show
/// over and over).
const DEEP_ROWS: i64 = 32;
/// Fewest columns the sea beside the picture repeats.
const MIN_BAND: i64 = 64;
/// A column with more bright pixels than this under the horizon has the moon's reflection in it; so have the
/// columns next to it that have any.
const GLITTER: usize = 4;

/// One monitor, in the coordinates of the whole desktop: logical pixels on Linux (as `hyprctl monitors` gives them),
/// physical ones on Windows. Only their proportions matter.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// `NAME:x,y,w,h;NAME:x,y,w,h;...`, the `--monitors` option.
pub fn parse_screens(spec: &str) -> Result<Vec<Screen>, String> {
    let mut out = Vec::new();
    for part in spec.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let bad = || format!("bad monitor {part:?}: expected NAME:x,y,w,h");
        let (name, geo) = part.rsplit_once(':').ok_or_else(bad)?;
        let v: Vec<f64> = geo
            .split(',')
            .map(|n| n.trim().parse::<f64>().map_err(|_| bad()))
            .collect::<Result<_, _>>()?;
        let [x, y, w, h] = v[..] else { return Err(bad()) };
        if !(w > 0.0 && h > 0.0 && x.is_finite() && y.is_finite()) {
            return Err(bad());
        }
        out.push(Screen {
            name: name.trim().to_string(),
            x,
            y,
            w,
            h,
        });
    }
    if out.is_empty() {
        return Err("--monitors lists no monitor".into());
    }
    Ok(out)
}

/// The monitors and which of them shows the picture.
#[derive(Clone, Debug)]
pub struct Layout {
    pub screens: Vec<Screen>,
    pub main: usize,
}

impl Layout {
    /// Where the 640x480 picture is on the main monitor: its top-left corner and the size of one of its pixels, in
    /// desktop coordinates. As large as fits, centred, as `present::scale` draws it.
    pub fn picture(&self) -> (f64, f64, f64) {
        let m = &self.screens[self.main];
        let k = (m.w / W as f64).min(m.h / H as f64);
        (m.x + (m.w - W as f64 * k) / 2.0, m.y + (m.h - H as f64 * k) / 2.0, k)
    }

    /// The monitor reaching farthest from the main one to the side, and whether that side is the right one.
    pub fn outermost(&self) -> Option<(usize, bool)> {
        let m = &self.screens[self.main];
        let centre = m.x + m.w / 2.0;
        self.screens
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != self.main)
            .map(|(i, s)| {
                let right = s.x + s.w / 2.0 > centre;
                let reach = if right { s.x + s.w - (m.x + m.w) } else { m.x - s.x };
                (i, right, reach)
            })
            .filter(|&(_, _, reach)| reach > 0.0)
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(i, right, _)| (i, right))
    }
}

/// The island of [`crate::engine::Engine::far_island`], shrunk and placed in picture coordinates beyond the
/// picture itself (the sea around it repeats the picture mirrored).
pub struct FarIsland {
    x: i64,
    y: i64,
    w: usize,
    h: usize,
    px: Vec<Option<u32>>,
}

impl FarIsland {
    /// Shrinks the island (palette indices, `None` where see-through) by averaging blocks of it; a block more than
    /// half see-through stays see-through. Placed on monitor `screen` of `layout`, near its outer edge
    /// (`right`), standing on the `horizon` row. `None` if the horizon is not on that monitor.
    pub fn new(
        layout: &Layout,
        (screen, right): (usize, bool),
        horizon: i32,
        island: &IslandPicture,
        pal: &[[u8; 3]; 256],
    ) -> Option<Self> {
        let (px0, py0, k) = layout.picture();
        let s = &layout.screens[screen];
        let base = py0 + (horizon as f64 + FAR_DROP as f64) * k;
        if base < s.y || base > s.y + s.h {
            return None;
        }
        let (iw, ipx) = (island.w, &island.px);
        let (w, h) = (iw / FAR_SHRINK, island.h / FAR_SHRINK);
        let mut px = Vec::with_capacity(w * h);
        for by in 0..h {
            for bx in 0..w {
                let (mut sum, mut n) = ([0u32; 3], 0usize);
                for y in by * FAR_SHRINK..(by + 1) * FAR_SHRINK {
                    for x in bx * FAR_SHRINK..(bx + 1) * FAR_SHRINK {
                        if let Some(i) = ipx[y * iw + x] {
                            let c = pal[i as usize];
                            for ch in 0..3 {
                                sum[ch] += c[ch] as u32;
                            }
                            n += 1;
                        }
                    }
                }
                px.push((n * 2 > FAR_SHRINK * FAR_SHRINK).then(|| {
                    let n = n as u32;
                    ((sum[0] / n) << 16) | ((sum[1] / n) << 8) | (sum[2] / n)
                }));
            }
        }
        let edge = if right {
            s.x + s.w * (1.0 - FAR_MARGIN)
        } else {
            s.x + s.w * FAR_MARGIN
        };
        let ex = ((edge - px0) / k).floor() as i64;
        Some(Self {
            x: if right { ex - w as i64 } else { ex },
            y: horizon as i64 + FAR_DROP - h as i64,
            w,
            h,
            px,
        })
    }

    /// Its colour over `under` at picture position (`u`, `v`), if it covers that spot.
    fn over(&self, u: i64, v: i64, under: u32) -> Option<u32> {
        let (x, y) = (u - self.x, v - self.y);
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return None;
        }
        let c = self.px[y as usize * self.w + x as usize]?;
        let mix = |sh: u32| (((c >> sh & 255) * (256 - FAR_HAZE) + (under >> sh & 255) * FAR_HAZE) >> 8) << sh;
        Some(mix(16) | mix(8) | mix(0))
    }
}

/// `p` folded into `0..n`, bouncing back at each end without repeating the end pixel, so the picture continues as
/// its mirror image and the dithering keeps its checkerboard.
fn mirror(p: i64, n: i64) -> i64 {
    if n < 2 {
        return 0;
    }
    let m = p.rem_euclid(2 * n - 2);
    if m < n { m } else { 2 * n - 2 - m }
}

/// The parts of the sea picture that the sea around it repeats: the sky above it, the deep sea below it, and
/// beside it the columns between its edge and the moon's reflection (which must not show twice).
pub struct Bands {
    /// Columns `0..left` to the left, `right..640` to the right.
    left: i64,
    right: i64,
    /// Rows `0..top` above, `bottom..480` below.
    top: i64,
    bottom: i64,
}

impl Bands {
    /// From the session's sea background (palette indices, no whitecaps) and its horizon row.
    pub fn new(bg: &[u8], pal: &[[u8; 3]; 256], horizon: i32) -> Self {
        let horizon = horizon as i64;
        if horizon <= 0 {
            return Self {
                left: W,
                right: 0,
                top: H,
                bottom: 0,
            };
        }
        let bright = |i: u8| pal[i as usize].iter().map(|&c| c as u32).sum::<u32>() > 600;
        let count: Vec<usize> = (0..W as usize)
            .map(|x| {
                (horizon as usize..H as usize)
                    .filter(|&y| bright(bg[y * W as usize + x]))
                    .count()
            })
            .collect();
        let (mut left, mut right) = match (
            count.iter().position(|&n| n > GLITTER),
            count.iter().rposition(|&n| n > GLITTER),
        ) {
            (Some(l), Some(r)) => (l, r + 1),
            _ => (W as usize, 0),
        };
        while left > 0 && left < W as usize && count[left - 1] > 0 {
            left -= 1;
        }
        while right > 0 && right < W as usize && count[right] > 0 {
            right += 1;
        }
        let (left, right) = (left as i64, right as i64);
        Self {
            left: if left >= MIN_BAND { left } else { W },
            right: if W - right >= MIN_BAND { right } else { 0 },
            top: horizon,
            bottom: H - DEEP_ROWS,
        }
    }

    fn col(&self, u: i64) -> usize {
        (match u {
            ..0 => mirror(u, self.left),
            W.. => self.right + mirror(u - self.right, W - self.right),
            _ => u,
        }) as usize
    }

    fn row(&self, v: i64) -> usize {
        (match v {
            ..0 => mirror(v, self.top),
            H.. => self.bottom + mirror(v - self.bottom, H - self.bottom),
            _ => v,
        }) as usize
    }
}

/// What the sea around the picture shows this session.
pub struct Surroundings {
    pub bands: Bands,
    pub far: Option<FarIsland>,
    /// The sea background as RGB, without whitecaps: the rows below the picture come from it.
    pub still: Vec<u32>,
}

/// For each of `n_dst` pixels spanning `len` desktop units from `from`: the picture coordinate it shows, before
/// mirroring.
fn axis(n_dst: usize, from: f64, len: f64, origin: f64, k: f64) -> Vec<i64> {
    (0..n_dst)
        .map(|i| ((from + (i as f64 + 0.5) * len / n_dst as f64 - origin) / k).floor() as i64)
        .collect()
}

/// Draws the sea (`sea`: a 640x480 RGB frame) into `dst`, the `w` x `h` pixels of monitor `screen` of `layout`,
/// leaving the pixels of `keep` (x, y, w, h in `dst`: the picture itself on the main monitor) alone.
pub fn draw(
    dst: &mut [u32],
    w: usize,
    h: usize,
    (layout, screen): (&Layout, usize),
    sea: &[u32],
    around: &Surroundings,
    keep: Option<(usize, usize, usize, usize)>,
) {
    let (px0, py0, k) = layout.picture();
    let s = &layout.screens[screen];
    let us = axis(w, s.x, s.w, px0, k);
    let vs = axis(h, s.y, s.h, py0, k);
    let cols: Vec<usize> = us.iter().map(|&u| around.bands.col(u)).collect();
    for (y, &v) in vs.iter().enumerate() {
        let source = if v >= H && around.still.len() == sea.len() {
            &around.still
        } else {
            sea
        };
        let row = &source[around.bands.row(v) * W as usize..][..W as usize];
        let line = &mut dst[y * w..][..w];
        let island_row = around.far.as_ref().filter(|f| v >= f.y && v < f.y + f.h as i64);
        let skip = keep.filter(|&(_, ky, _, kh)| y >= ky && y < ky + kh);
        for (x, d) in line.iter_mut().enumerate() {
            if skip.is_some_and(|(kx, _, kw, _)| x >= kx && x < kx + kw) {
                continue;
            }
            let c = row[cols[x]];
            *d = island_row.and_then(|f| f.over(us[x], v, c)).unwrap_or(c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitors_parse() {
        let s = parse_screens("DP-3:-960,-704,1920,1080; HDMI-A-1:960,-540,900,1600").unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].name, "HDMI-A-1");
        assert_eq!((s[1].x, s[1].w), (960.0, 900.0));
        assert!(parse_screens("DP-3:1,2,3").is_err());
        assert!(parse_screens("DP-3:1,2,0,4").is_err());
        assert!(parse_screens("").is_err());
    }

    #[test]
    fn mirror_bounces_at_both_ends() {
        assert_eq!(mirror(0, 640), 0);
        assert_eq!(mirror(639, 640), 639);
        assert_eq!(mirror(640, 640), 638);
        assert_eq!(mirror(641, 640), 637);
        assert_eq!(mirror(-1, 640), 1);
        assert_eq!(mirror(-2, 640), 2);
        assert_eq!(mirror(1278, 640), 0);
    }

    #[test]
    fn picture_is_centred_and_outermost_is_found() {
        let layout = Layout {
            screens: parse_screens("A:0,0,1920,1080;B:1920,0,900,1600;C:-1280,0,1280,1024").unwrap(),
            main: 0,
        };
        let (x, y, k) = layout.picture();
        assert_eq!((x, y, k), (240.0, 0.0, 2.25));
        assert_eq!(layout.outermost(), Some((2, false)));
        let one = Layout {
            screens: parse_screens("A:0,0,1920,1080").unwrap(),
            main: 0,
        };
        assert_eq!(one.outermost(), None);
    }

    #[test]
    fn sea_continues_the_picture_edge() {
        // A sea whose columns are numbered: right of the picture it comes back mirrored.
        let sea: Vec<u32> = (0..WIDTH as u32 * HEIGHT as u32).map(|i| i % WIDTH as u32).collect();
        let layout = Layout {
            screens: parse_screens("A:0,0,640,480;B:640,0,640,480").unwrap(),
            main: 0,
        };
        let around = Surroundings {
            bands: Bands {
                left: 640,
                right: 400,
                top: 150,
                bottom: 384,
            },
            far: None,
            still: Vec::new(),
        };
        let mut dst = vec![0; 640 * 480];
        draw(&mut dst, 640, 480, (&layout, 1), &sea, &around, None);
        assert_eq!(&dst[..3], &[638, 637, 636]);
        // Bounces back at the moon's reflection (column 400).
        assert_eq!(dst[238..241], [400, 401, 402]);
    }
}
