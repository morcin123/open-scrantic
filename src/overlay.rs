//! Clock and weather drawn over the picture, at the top of the screen. Not in the original: this fork's
//! addition, presentation only like the rest of `present.rs`. Text is drawn in screen pixels after scaling, so it
//! stays sharp at any resolution. Labels are in Polish.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Datelike, Local, Timelike};
use fontdue::{Font, FontSettings};

use crate::engine::config::Config;

const BOLD: &[u8] = include_bytes!("../resources/fonts/JetBrainsMonoNerdFont-Bold.ttf");
const REGULAR: &[u8] = include_bytes!("../resources/fonts/JetBrainsMonoNerdFont-Regular.ttf");

/// How often the weather is fetched again while the screensaver runs.
const WEATHER_EVERY: Duration = Duration::from_secs(15 * 60);
/// A failed fetch is retried sooner than a successful one is refreshed.
const RETRY_EVERY: Duration = Duration::from_secs(60);

const WEEKDAYS: [&str; 7] = [
    "poniedziałek",
    "wtorek",
    "środa",
    "czwartek",
    "piątek",
    "sobota",
    "niedziela",
];
const MONTHS: [&str; 12] = [
    "stycznia",
    "lutego",
    "marca",
    "kwietnia",
    "maja",
    "czerwca",
    "lipca",
    "sierpnia",
    "września",
    "października",
    "listopada",
    "grudnia",
];

/// Current conditions, from wttr.in's one-line format `%t|%x|%l|%S|%s`.
#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub temp_c: i32,
    /// wttr.in's plain-text condition symbol (`o` sunny, `mm` cloudy, `/` light rain, ...).
    pub symbol: String,
    /// Sunrise and sunset, minutes after midnight, local to the place.
    pub sunrise: u32,
    pub sunset: u32,
}

/// `07:10:57` (or `07:10`) as minutes after midnight.
fn minutes(text: &str) -> Option<u32> {
    let mut parts = text.trim().split(':');
    let h: u32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Parses `+17°C|o|Warszawa|07:10:57|17:30:00`.
pub fn parse_report(text: &str) -> Option<Weather> {
    let fields: Vec<&str> = text.trim().split('|').collect();
    let [temp, symbol, _place, sunrise, sunset] = fields[..] else {
        return None;
    };
    let digits: String = temp
        .trim()
        .chars()
        .take_while(|c| *c == '+' || *c == '-' || c.is_ascii_digit())
        .collect();
    Some(Weather {
        temp_c: digits.trim_start_matches('+').parse().ok()?,
        symbol: symbol.trim().to_string(),
        sunrise: minutes(sunrise)?,
        sunset: minutes(sunset)?,
    })
}

/// Nerd Font icon (day, night) and Polish label for a wttr.in condition symbol. The icons are the ones the
/// Omarchy weather widget uses for the same conditions.
fn condition(symbol: &str) -> (char, char, &'static str) {
    match symbol {
        "o" => ('\u{e30d}', '\u{e32b}', "Bezchmurnie"),
        "m" => ('\u{e302}', '\u{e32e}', "Częściowe zachmurzenie"),
        "mm" => ('\u{e33d}', '\u{e33d}', "Pochmurno"),
        "mmm" => ('\u{e33d}', '\u{e33d}', "Całkowite zachmurzenie"),
        "=" => ('\u{e313}', '\u{e346}', "Mgła"),
        "." => ('\u{e308}', '\u{e333}', "Przelotny deszcz"),
        "/" => ('\u{e318}', '\u{e318}', "Lekki deszcz"),
        "//" => ('\u{e318}', '\u{e318}', "Silne przelotne opady"),
        "///" => ('\u{e318}', '\u{e318}', "Ulewa"),
        "x" | "x/" => ('\u{e3ad}', '\u{e3ad}', "Deszcz ze śniegiem"),
        "*" | "*/" => ('\u{e30a}', '\u{e327}', "Lekki śnieg"),
        "**" | "*/*" => ('\u{e31a}', '\u{e31a}', "Śnieg"),
        "/!/" | "!/" | "*!*" => ('\u{e31d}', '\u{e31d}', "Burza"),
        _ => ('\u{e33d}', '\u{e33d}', ""),
    }
}

/// Percent-encodes `s` for a URL path segment; `,` is kept so `lat,lon` stays coordinates.
fn encode_path(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// GET `url` with the system's `curl` (shipped with Windows 10 and later, and with every Linux and macOS); the body
/// on success.
fn curl(url: &str, seconds: u32) -> Option<String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-fsS", "--max-time", &seconds.to_string(), url]);
    // The screensaver has no console: without this flag Windows would flash one up for curl.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd.output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// One weather fetch for `place`: a name, `lat,lon`, or empty for the place of the IP address.
fn fetch(place: &str) -> Option<Weather> {
    parse_report(&curl(
        &format!("https://wttr.in/{}?format=%t|%x|%l|%S|%s&m", encode_path(place)),
        15,
    )?)
}

/// A place found by [`search_places`].
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    /// District, province and country, to tell apart the many places that share a name.
    pub description: String,
    pub latitude: f64,
    pub longitude: f64,
}

impl Place {
    /// `lat,lon`, as `WeatherCoords` stores it.
    pub fn coords(&self) -> String {
        crate::engine::config::clean_coords(&format!("{},{}", self.latitude, self.longitude))
    }
}

/// Places in an Open-Meteo geocoding reply, Polish administrative words shortened.
pub fn parse_places(json: &str) -> Vec<Place> {
    let Ok(reply) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(results) = reply["results"].as_array() else {
        return Vec::new();
    };
    results
        .iter()
        .filter_map(|r| {
            let name = r["name"].as_str()?.to_string();
            let description = ["admin2", "admin1", "country"]
                .iter()
                .filter_map(|key| r[*key].as_str())
                .filter(|part| !part.is_empty() && *part != name)
                .map(|part| {
                    let short = |prefix: &str, with: &str| {
                        part.strip_prefix(prefix)
                            .or_else(|| part.strip_prefix(&prefix.to_lowercase()))
                            .map(|rest| format!("{with}{rest}"))
                    };
                    short("Województwo ", "woj. ")
                        .or_else(|| short("Powiat ", "pow. "))
                        .unwrap_or_else(|| part.to_string())
                })
                .collect::<Vec<_>>()
                .join(", ");
            Some(Place {
                name,
                description,
                latitude: r["latitude"].as_f64()?,
                longitude: r["longitude"].as_f64()?,
            })
        })
        .collect()
}

/// Places matching `query`, with Polish names (Open-Meteo geocoding, GeoNames data: small villages included).
pub fn search_places(query: &str) -> Vec<Place> {
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=8&language=pl&format=json",
        encode_path(query.trim())
    );
    curl(&url, 8).map_or_else(Vec::new, |body| parse_places(&body))
}

/// Starts fetching the weather for `place` in the background; the latest report shows up in the returned slot.
fn spawn_weather(place: String) -> Arc<Mutex<Option<Weather>>> {
    let slot = Arc::new(Mutex::new(None));
    let writer = Arc::clone(&slot);
    std::thread::spawn(move || {
        loop {
            let report = fetch(&place);
            let ok = report.is_some();
            if ok && let Ok(mut s) = writer.lock() {
                *s = report;
            }
            std::thread::sleep(if ok { WEATHER_EVERY } else { RETRY_EVERY });
        }
    });
    slot
}

/// `text` drawn in `font` at `size` pixels: its width and the glyphs, with their offsets.
struct Run {
    width: f32,
    glyphs: Vec<(f32, fontdue::Metrics, Vec<u8>)>,
}

fn layout(font: &Font, text: &str, size: f32) -> Run {
    let mut x = 0.0;
    let mut glyphs = Vec::new();
    for c in text.chars() {
        let (m, bitmap) = font.rasterize(c, size);
        glyphs.push((x, m, bitmap));
        x += m.advance_width;
    }
    Run { width: x, glyphs }
}

/// The laid-out box: where it sits on the screen and how much white text covers each of its pixels.
struct Picture {
    key: String,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    /// Per pixel of the box: 0 transparent .. 255 full white.
    ink: Vec<u8>,
}

impl Picture {
    /// Adds `run` with its left end at `x` and baseline at `baseline`, at `alpha` (0..=255).
    fn stamp(&mut self, run: &Run, x: f32, baseline: f32, alpha: u32) {
        for (gx, m, bitmap) in &run.glyphs {
            let left = (x + gx + m.xmin as f32).round() as i64;
            let top = (baseline - m.height as f32 - m.ymin as f32).round() as i64;
            for row in 0..m.height {
                for col in 0..m.width {
                    let (px, py) = (left + col as i64, top + row as i64);
                    if px < 0 || py < 0 || px >= self.w as i64 || py >= self.h as i64 {
                        continue;
                    }
                    let cov = bitmap[row * m.width + col] as u32 * alpha / 255;
                    let dst = &mut self.ink[py as usize * self.w + px as usize];
                    *dst = (*dst).max(cov as u8);
                }
            }
        }
    }
}

pub struct Overlay {
    clock: bool,
    weather: Option<Arc<Mutex<Option<Weather>>>>,
    /// The place as the user typed it; shown after the conditions (nothing for a place found from the IP).
    place: String,
    bold: Font,
    regular: Font,
    picture: Option<Picture>,
}

impl Overlay {
    /// The overlay `cfg` asks for, or `None` when both the clock and the weather are off. Starts fetching the
    /// weather.
    pub fn new(cfg: &Config) -> Option<Self> {
        if !cfg.clock && !cfg.weather {
            return None;
        }
        let bold = Font::from_bytes(BOLD, FontSettings::default()).ok()?;
        let regular = Font::from_bytes(REGULAR, FontSettings::default()).ok()?;
        Some(Self {
            clock: cfg.clock,
            // Coordinates of a place picked from the search are exact; a typed name is looked up by wttr.in.
            weather: cfg.weather.then(|| {
                spawn_weather(if cfg.weather_coords.is_empty() {
                    cfg.weather_location.clone()
                } else {
                    cfg.weather_coords.clone()
                })
            }),
            place: cfg.weather_location.clone(),
            bold,
            regular,
            picture: None,
        })
    }

    /// The two lines of text now: the big one (time, icon, temperature) and the small one (date, conditions,
    /// place).
    fn lines(&self) -> (String, Option<(char, String)>, String) {
        let now = Local::now();
        let time = if self.clock {
            format!("{:02}:{:02}", now.hour(), now.minute())
        } else {
            String::new()
        };
        let weather = self
            .weather
            .as_ref()
            .and_then(|slot| slot.lock().ok().and_then(|w| w.clone()));
        let mut small = Vec::new();
        if self.clock {
            let weekday = WEEKDAYS[now.weekday().num_days_from_monday() as usize];
            small.push(format!("{weekday}, {} {}", now.day(), MONTHS[now.month0() as usize]));
        }
        let big_weather = weather.map(|w| {
            let minute = now.hour() * 60 + now.minute();
            let night = minute < w.sunrise || minute >= w.sunset;
            let (day_icon, night_icon, label) = condition(&w.symbol);
            if !label.is_empty() {
                small.push(label.to_string());
            }
            if !self.place.is_empty() {
                small.push(self.place.clone());
            }
            (if night { night_icon } else { day_icon }, format!("{}°", w.temp_c))
        });
        (time, big_weather, small.join("  ·  "))
    }

    /// Lays the box out for a `w` x `h` screen.
    fn render(&self, key: String, w: usize, h: usize) -> Option<Picture> {
        let (time, weather, small) = self.lines();
        if time.is_empty() && weather.is_none() {
            return None;
        }
        let big = (h as f32 * 0.065).max(18.0);
        let little = (big * 0.34).max(11.0);
        let gap = big * 0.35;

        let time_run = layout(&self.bold, &time, big);
        let weather_runs = weather.as_ref().map(|(icon, temp)| {
            (
                layout(&self.regular, &icon.to_string(), big * 0.85),
                layout(&self.bold, temp, big),
            )
        });
        let small_run = layout(&self.regular, &small, little);

        let divider = !time.is_empty() && weather_runs.is_some();
        let mut top_width = time_run.width;
        if let Some((icon, temp)) = &weather_runs {
            if divider {
                top_width += gap * 2.0;
            }
            top_width += icon.width + gap * 0.9 + temp.width;
        }
        let pad_x = big * 0.45;
        let pad_y = big * 0.22;
        let line = self.bold.horizontal_line_metrics(big)?;
        let small_line = self.regular.horizontal_line_metrics(little)?;
        let content_w = top_width.max(small_run.width);
        let box_w = (content_w + pad_x * 2.0).ceil() as usize;
        let small_h = if small.is_empty() {
            0.0
        } else {
            small_line.new_line_size
        };
        let box_h = (pad_y * 2.0 + line.ascent - line.descent + small_h).ceil() as usize;
        if box_w > w || box_h > h {
            return None;
        }
        let mut pic = Picture {
            key,
            x: (w - box_w) / 2,
            y: (h as f32 * 0.03) as usize,
            w: box_w,
            h: box_h,
            ink: vec![0; box_w * box_h],
        };
        let baseline = pad_y + line.ascent;
        let mut x = (box_w as f32 - top_width) / 2.0;
        pic.stamp(&time_run, x, baseline, 255);
        x += time_run.width;
        if let Some((icon, temp)) = &weather_runs {
            if divider {
                let (dx, top) = ((x + gap) as usize, (pad_y + line.ascent * 0.15) as usize);
                let bottom = (baseline - line.descent * 0.2) as usize;
                for y in top..bottom.min(box_h) {
                    if dx < box_w {
                        pic.ink[y * box_w + dx] = 90;
                    }
                }
                x += gap * 2.0;
            }
            pic.stamp(icon, x, baseline - big * 0.04, 255);
            x += icon.width + gap * 0.9;
            pic.stamp(temp, x, baseline, 255);
        }
        if !small.is_empty() {
            let small_baseline = baseline - line.descent + small_line.ascent;
            pic.stamp(&small_run, (box_w as f32 - small_run.width) / 2.0, small_baseline, 190);
        }
        Some(pic)
    }

    /// Draws the box over `buf`, a `w` x `h` frame of packed RGB: the area darkened, the text in white.
    pub fn draw(&mut self, buf: &mut [u32], w: usize, h: usize) {
        let (time, weather, small) = self.lines();
        let key = format!("{w}x{h}|{time}|{weather:?}|{small}");
        if self.picture.as_ref().is_none_or(|p| p.key != key) {
            self.picture = self.render(key, w, h);
        }
        let Some(pic) = &self.picture else { return };
        for row in 0..pic.h {
            let line = &mut buf[(pic.y + row) * w + pic.x..][..pic.w];
            for (px, &ink) in line.iter_mut().zip(&pic.ink[row * pic.w..][..pic.w]) {
                // Half-dark backdrop, then white text on it.
                let dark = (*px >> 1) & 0x7F_7F7F;
                let a = ink as u32;
                let ch = |sh: u32| {
                    let c = dark >> sh & 0xFF;
                    (c + (255 - c) * a / 255) << sh
                };
                *px = ch(16) | ch(8) | ch(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_parse_and_bad_ones_are_rejected() {
        let w = parse_report("+17°C|o|Warszawa|07:10:57|17:30:00\n").unwrap();
        assert_eq!(
            w,
            Weather {
                temp_c: 17,
                symbol: "o".into(),
                sunrise: 7 * 60 + 10,
                sunset: 17 * 60 + 30
            }
        );
        assert_eq!(parse_report("-3°C|*|x|07:10|17:30").unwrap().temp_c, -3);
        for bad in [
            "",
            "Unknown location",
            "+17°C|o|W|07:10:57",
            "abc|o|W|07:10|17:30",
            "+1°C|o|W|25:00|17:30",
        ] {
            assert!(parse_report(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn places_parse_with_short_district_names() {
        let reply = r#"{"results":[
            {"name":"Wola","admin2":"Powiat pszczyński","admin1":"Województwo śląskie","country":"Polska",
             "latitude":50.01745,"longitude":19.12333},
            {"name":"Nowy Sącz","admin2":"Nowy Sącz","admin1":"województwo małopolskie","country":"Polska",
             "latitude":49.62177,"longitude":20.69705},
            {"name":"No coordinates"}
        ]}"#;
        let places = parse_places(reply);
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].description, "pow. pszczyński, woj. śląskie, Polska");
        assert_eq!(places[1].description, "woj. małopolskie, Polska");
        assert_eq!(places[1].coords(), "49.62177,20.69705");
        assert!(parse_places("{}").is_empty());
        assert!(parse_places("not json").is_empty());
    }

    /// Real requests, run only with `OPEN_SCRANTIC_NETWORK` set (the Windows screensaver workflow sets it, which
    /// checks curl.exe and the encoding of Polish names there).
    #[test]
    fn live_search_and_weather_for_a_picked_place() {
        if std::env::var_os("OPEN_SCRANTIC_NETWORK").is_none() {
            return;
        }
        let places = search_places("Nowy Sącz");
        let place = places
            .iter()
            .find(|p| p.name == "Nowy Sącz" && p.description.contains("małopolskie"))
            .unwrap_or_else(|| panic!("{places:?}"));
        assert!(fetch(&place.coords()).is_some(), "no weather for {}", place.coords());
    }

    #[test]
    fn locations_are_encoded_for_the_url() {
        assert_eq!(encode_path("Warszawa"), "Warszawa");
        assert_eq!(encode_path("52.23,21.01"), "52.23,21.01");
        assert_eq!(encode_path("Nowy Sącz"), "Nowy%20S%C4%85cz");
        assert_eq!(encode_path(""), "");
    }

    #[test]
    fn every_symbol_has_an_icon_the_font_has() {
        let font = Font::from_bytes(REGULAR, FontSettings::default()).unwrap();
        for s in [
            "o", "m", "mm", "mmm", "=", ".", "/", "//", "///", "x", "*", "**", "!/", "?",
        ] {
            let (day, night, _) = condition(s);
            assert!(font.has_glyph(day) && font.has_glyph(night), "{s}");
        }
    }

    #[test]
    fn the_box_is_centered_at_the_top_and_inks_text() {
        let overlay = Overlay {
            clock: true,
            weather: Some(Arc::new(Mutex::new(parse_report("+17°C|o|W|07:10|17:30")))),
            place: "Warszawa".into(),
            bold: Font::from_bytes(BOLD, FontSettings::default()).unwrap(),
            regular: Font::from_bytes(REGULAR, FontSettings::default()).unwrap(),
            picture: None,
        };
        let (w, h) = (1920, 1080);
        let pic = overlay.render(String::new(), w, h).unwrap();
        assert!((pic.x + pic.w / 2).abs_diff(w / 2) <= 1);
        assert!(pic.y < h / 10 && pic.w < w / 2);
        assert!(pic.ink.contains(&255));
        // Too small a screen for the box: nothing is drawn rather than a clipped box.
        assert!(overlay.render(String::new(), 40, 40).is_none());
    }

    #[test]
    fn drawing_darkens_the_box_only() {
        let mut overlay = Overlay::new(&Config {
            weather: false,
            ..Config::default()
        })
        .unwrap();
        let (w, h) = (1280, 720);
        let mut buf = vec![0x00FF_FFFF; w * h];
        overlay.draw(&mut buf, w, h);
        assert_eq!(buf[h - 1], 0x00FF_FFFF);
        let pic = overlay.picture.as_ref().unwrap();
        let corner = buf[pic.y * w + pic.x];
        assert_eq!(corner, 0x007F_7F7F);
    }
}
