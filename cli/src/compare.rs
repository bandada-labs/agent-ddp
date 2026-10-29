//! `ddp compare`: how far what the app shows is from a reference image, or from the same element on the other phone.
//! A measurement for the agent to judge, never a problem report, so it exits 0 whatever the difference. Only a bar
//! the caller sets (`--max`, `--max-band`) makes it exit 1, and the bar is theirs, not a judgment of ddp's.
//!
//! Where the pixels come from: the whole screen is the device's own screenshot (status bar included, like a reference
//! screenshot); an element is the app's own windows as composited (`screen.pixels`), cut to the element's frame.
use crate::args::Args;
use crate::device::{png_read, png_write, screenshot_png, target};
use crate::inspect::{base64, SAME};
use crate::live::{open, snapshot_live};
use crate::model::rect;
use crate::py;
use crate::state::{self, failure, read_json, write_file, Result};
use crate::{fail, say};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The screen is judged in this many bands from top to bottom, an element in fewer.
const SCREEN_BANDS: usize = 8;
const ELEMENT_BANDS: usize = 4;
/// The picture written for a person or an agent to look at stays under this width; the scores use every pixel.
const MAX_PICTURE_WIDTH: usize = 1800;

pub fn compare(args: &mut Args) -> Result<i32> {
    let out = PathBuf::from(&args.out);
    let bar = Bar { overall: share(args.max.as_deref(), "max")?, band: share(args.max_band.as_deref(), "max-band")? };
    let difference = match (&args.id, &args.reference, &args.on) {
        _ if args.screen.is_some() && args.id.is_some() => {
            fail!(
                "--screen is a whole screenshot, not an element",
                "bad_value",
                "ddp compare --reference a.png --screen b.png"
            )
        }
        (None, Some(reference), _) => screen_against(args, reference, &out),
        _ if !args.masks.is_empty() => {
            fail!(
                "--mask is for the whole screen",
                "bad_value",
                "with an element, leave parts out with --ignore ELEMENT"
            )
        }
        (Some(key), Some(reference), _) => element_against(key, reference, &args.ignore, &out),
        (Some(key), None, Some(on)) => element_across(key, on, &args.ignore, &out),
        _ => fail!(
            "compare needs --reference, or an element and --on",
            "missing_option",
            "ddp compare --reference ref.png, or ddp compare card --on ios,android"
        ),
    }?;
    Ok(bar.judge(&difference))
}

/// `5`, `5%` or `2.5%` as a share of 0..1.
fn share(text: Option<&str>, option: &str) -> Result<Option<f64>> {
    let Some(text) = text else { return Ok(None) };
    match text.trim().trim_end_matches('%').parse::<f64>() {
        Ok(percent) if (0.0..=100.0).contains(&percent) => Ok(Some(percent / 100.0)),
        _ => fail!(
            format!("--{option} takes a percentage from 0 to 100, not '{text}'"),
            "bad_value",
            format!("ddp compare --reference ref.png --{option} 5%")
        ),
    }
}

/// The caller's pass bar: the most pixels that may differ, overall and in any one band.
struct Bar {
    overall: Option<f64>,
    band: Option<f64>,
}

impl Bar {
    /// Prints which part is over the bar; 1 when one is, so a run over many screens can count passes.
    fn judge(&self, difference: &Difference) -> i32 {
        if self.overall.is_none() && self.band.is_none() {
            return 0;
        }
        let mut over = Vec::new();
        if let Some(bar) = self.overall.filter(|bar| difference.share_off > *bar) {
            over.push(format!(
                "over --max: {} of the pixels differ, the bar is {}",
                tenths(difference.share_off),
                tenths(bar)
            ));
        }
        if let Some(bar) = self.band {
            for band in difference.bands.iter().filter(|band| band.share_off > bar) {
                over.push(format!(
                    "over --max-band: band {}-{} differs {}, the bar is {}",
                    band.top,
                    band.bottom,
                    tenths(band.share_off),
                    tenths(bar)
                ));
            }
        }
        if over.is_empty() {
            say!("within the bar");
            return 0;
        }
        over.iter().for_each(|line| say!("{line}"));
        1
    }
}

/// The device's screen against a reference screenshot of another size: the reference is scaled to the screen's
/// width, top edges together, and only the height both have is compared.
fn screen_against(args: &Args, reference_path: &Path, out: &Path) -> Result<Difference> {
    let screen = match &args.screen {
        Some(saved) if !args.ignore.is_empty() => {
            fail!(
                format!("--ignore needs the live screen, and {} is a saved one", saved.display()),
                "bad_value",
                "with --screen, leave parts out with --mask X,Y,W,H"
            )
        }
        Some(saved) => read_image(saved)?,
        None => {
            let (platform, device, _) = target(args)?;
            Image::from_png(&screenshot_png(&json!({"platform": platform, "device": device}))?)?
        }
    };
    let reference = read_image(reference_path)?;
    let factor = screen.width as f64 / reference.width as f64;
    let scaled = reference.resized(screen.width, (reference.height as f64 * factor).round() as usize);
    let height = scaled.height.min(screen.height);
    let (scaled, screen) = (scaled.top(height), screen.top(height));
    let mut mask = args.masks.iter().map(|text| pixel_rect(text)).collect::<Result<Vec<_>>>()?;
    if !args.ignore.is_empty() {
        mask.extend(on_screen(&args.ignore)?);
    }
    let name = args.screen.as_ref().map_or("screen".to_string(), |saved| saved.display().to_string());
    say!(
        "{name} {}x{} against {} {}x{} (scaled {})",
        screen.width,
        screen.height,
        reference_path.display(),
        reference.width,
        reference.height,
        py::float(py::round_f(factor, 3))
    );
    report(&scaled, &screen, SCREEN_BANDS, ["reference", &name], &mask, out)
}

/// Where the elements are on the device's screenshot: points times the scale on iOS, pixels from the activity
/// window's place on the screen on Android.
fn on_screen(keys: &[String]) -> Result<Vec<[f64; 4]>> {
    let session = crate::live::require_session()?;
    let recording = snapshot_live(&session)?;
    let frame = recording.frames.last().unwrap();
    let (scale, origin) = if session["platform"] == "ios" {
        (recording.header.get("scale").map(py::f).unwrap_or(1.0), [0.0, 0.0])
    } else {
        let mut control = open(&session)?;
        let answer = control.call("screen.hit", json!({"id": keys[0]}))?;
        control.close();
        (1.0, [py::f(&answer["origin"][0]), py::f(&answer["origin"][1])])
    };
    keys.iter()
        .map(|key| {
            let Some(element) = frame.element(key) else {
                fail!(format!("no element '{key}' on screen"), "unknown_element", "ddp tree -i");
            };
            let [x, y, w, h] = rect(&element["frame"]);
            Ok([(x + origin[0]) * scale, (y + origin[1]) * scale, w * scale, h * scale])
        })
        .collect()
}

/// `x,y,w,h` in the screenshot's pixels.
fn pixel_rect(text: &str) -> Result<[f64; 4]> {
    let numbers: Vec<f64> = text.split(',').filter_map(|n| n.trim().parse().ok()).collect();
    match numbers[..] {
        [x, y, w, h] if w > 0.0 && h > 0.0 => Ok([x, y, w, h]),
        _ => fail!(
            format!("--mask takes x,y,w,h in pixels, not '{text}'"),
            "bad_value",
            "ddp compare --reference ref.png --mask 0,0,1206,162"
        ),
    }
}

/// One element of the app against a reference image of it, both at the smaller of the two sizes.
fn element_against(key: &str, reference_path: &Path, ignore: &[String], out: &Path) -> Result<Difference> {
    let session = crate::live::require_session()?;
    let piece = element_pixels(&session, key, ignore)?;
    let element = &piece.image;
    let reference = read_image(reference_path)?;
    say!(
        "{key} {}x{} px against {} {}x{}",
        element.width,
        element.height,
        reference_path.display(),
        reference.width,
        reference.height
    );
    let (a, b) = same_size(&reference, element);
    report(&a, &b, ELEMENT_BANDS, ["reference", key], &piece.mask(a.width, a.height), out)
}

/// One element on two sessions (two phones), both at the smaller of the two sizes.
fn element_across(key: &str, on: &str, ignore: &[String], out: &Path) -> Result<Difference> {
    let names: Vec<&str> = on.split(',').map(str::trim).collect();
    let [first, second] = names[..] else {
        fail!("--on takes two sessions, e.g. ios,android", "bad_value", "ddp compare card --on ios,android");
    };
    let pixels = |name: &str| -> Result<Piece> {
        let session_file = Path::new(".ddp").join("sessions").join(name).join("session.json");
        let Some(session) = read_json(&session_file) else {
            fail!(
                format!("no session '{name}'"),
                "no_session",
                format!("ddp connect DEVICE --app ID --session {name}")
            );
        };
        element_pixels(&session, key, ignore)
    };
    let (one, other) = (pixels(first)?, pixels(second)?);
    let (a, b) = (&one.image, &other.image);
    say!("{key} on {first} {}x{} px against {second} {}x{} px", a.width, a.height, b.width, b.height);
    let (a, b) = same_size(a, b);
    // An ignored part sits a little differently on each phone: both places are left out.
    let mask = [one.mask(a.width, a.height), other.mask(a.width, a.height)].concat();
    report(&a, &b, ELEMENT_BANDS, [first, second], &mask, out)
}

/// An element's pixels, its frame, and the frames of the parts of it to leave out.
struct Piece {
    image: Image,
    frame: [f64; 4],
    ignored: Vec<[f64; 4]>,
}

impl Piece {
    /// The ignored parts in the pixels of the element's image once resized to `width` x `height`.
    fn mask(&self, width: usize, height: usize) -> Vec<[f64; 4]> {
        let [left, top, w, h] = self.frame;
        let (sx, sy) = (width as f64 / w.max(f64::EPSILON), height as f64 / h.max(f64::EPSILON));
        self.ignored.iter().map(|[x, y, w, h]| [(x - left) * sx, (y - top) * sy, w * sx, h * sy]).collect()
    }
}

/// The element's pixels as the app's windows show them now.
fn element_pixels(session: &Value, key: &str, ignore: &[String]) -> Result<Piece> {
    let recording = snapshot_live(session)?;
    let frame = recording.frames.last().unwrap();
    let place = |key: &str| match frame.element(key) {
        Some(element) => Ok(rect(&element["frame"])),
        None => Err(failure(format!("no element '{key}' on screen"), "unknown_element", Some("ddp tree -i"))),
    };
    let element_frame = place(key)?;
    let ignored = ignore.iter().map(|key| place(key)).collect::<Result<Vec<_>>>()?;
    let mut control = open(session)?;
    let answer = control.call("screen.pixels", json!({"rects": [element_frame]}))?;
    control.close();
    let region = &answer["regions"][0];
    if let Some(missing) = region["missing"].as_str() {
        fail!(format!("the app gave no pixels for {key}: {missing}"), "no_pixels", "ddp doctor");
    }
    let (width, height) =
        (region["width"].as_u64().unwrap_or(0) as usize, region["height"].as_u64().unwrap_or(0) as usize);
    let rgba = region["rgba"].as_str().and_then(base64).unwrap_or_default();
    if width == 0 || height == 0 || rgba.len() != width * height * 4 {
        fail!(format!("the app gave no pixels for {key}"), "no_pixels", "ddp doctor");
    }
    Ok(Piece { image: Image { width, height, rgba }, frame: element_frame, ignored })
}

/// Both images at the smaller width and height of the two, so neither is enlarged more than it must be.
fn same_size(a: &Image, b: &Image) -> (Image, Image) {
    let (width, height) = (a.width.min(b.width), a.height.min(b.height));
    (a.resized(width, height), b.resized(width, height))
}

/// A PNG, or any image macOS can read (JPEG, WebP, HEIC), through `sips`.
fn read_image(path: &Path) -> Result<Image> {
    let data = std::fs::read(path).map_err(|e| {
        failure(
            format!("can't read {}: {e}", path.display()),
            "file_not_found",
            Some("ddp compare --reference ref.png"),
        )
    })?;
    if let Ok(image) = Image::from_png(&data) {
        return Ok(image);
    }
    let converted = state::scratch_file("reference.png");
    let result = state::capture(
        "sips",
        &["-s", "format", "png", &path.display().to_string(), "--out", &converted.display().to_string()],
    );
    let image = (result.code == 0).then(|| std::fs::read(&converted).ok()).flatten().map(|data| Image::from_png(&data));
    match image {
        Some(Ok(image)) => Ok(image),
        _ => fail!(
            format!("can't read {} as an image", path.display()),
            "bad_png",
            "an 8-bit PNG, or on macOS any image sips can open"
        ),
    }
}

/// Prints the difference overall and per band, and writes the two images side by side with the difference.
fn report(a: &Image, b: &Image, bands: usize, names: [&str; 2], mask: &[[f64; 4]], out: &Path) -> Result<Difference> {
    let difference = Difference::of(a, b, bands, mask);
    say!("difference {} (pixels that differ: {})", tenths(difference.mean), tenths(difference.share_off));
    if !mask.is_empty() {
        say!("left out: {} of the pixels (dark gray in the picture)", tenths(difference.left_out));
    }
    say!("by band, top to bottom ({} px each):", a.height.div_ceil(bands));
    for band in &difference.bands {
        say!("  {:>5}-{:<5} {:>6}  differ {}", band.top, band.bottom, tenths(band.mean), tenths(band.share_off));
    }
    let picture = Image::side_by_side(&[a, b, &difference.image]).fit(MAX_PICTURE_WIDTH);
    write_file(out, png_write(picture.width, picture.height, &picture.rows()))?;
    say!("{}: {} | {} | difference (red: differs)", out.display(), names[0], names[1]);
    Ok(difference)
}

fn tenths(share: f64) -> String {
    format!("{:.1}%", share * 100.0)
}

struct Band {
    top: usize,
    bottom: usize,
    mean: f64,
    share_off: f64,
}

struct Difference {
    /// The mean over every pixel of its largest channel difference, 0..1.
    mean: f64,
    /// The share of pixels whose colors are not the same color on screen (more than `SAME` apart).
    share_off: f64,
    /// The share of pixels left out by the mask, which count in no score.
    left_out: f64,
    bands: Vec<Band>,
    image: Image,
}

impl Difference {
    fn of(a: &Image, b: &Image, bands: usize, mask: &[[f64; 4]]) -> Difference {
        let (width, height) = (a.width, a.height);
        let mut image = Image { width, height, rgba: vec![0; width * height * 4] };
        let masked = |x: usize, y: usize| {
            let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
            mask.iter().any(|[left, top, w, h]| x >= *left && x < left + w && y >= *top && y < top + h)
        };
        // Per row: the sum of differences, the count of pixels off and of pixels counted, so bands are sums of rows.
        let mut rows = Vec::with_capacity(height);
        for y in 0..height {
            let (mut sum, mut off, mut counted) = (0.0, 0usize, 0usize);
            for x in 0..width {
                let at = (y * width + x) * 4;
                if masked(x, y) {
                    image.rgba[at..at + 4].copy_from_slice(&[80, 80, 80, 255]);
                    continue;
                }
                counted += 1;
                let (p, q) = (&a.rgba[at..at + 3], &b.rgba[at..at + 3]);
                let channel = (0..3).map(|c| (p[c] as i32 - q[c] as i32).abs()).max().unwrap();
                sum += channel as f64 / 255.0;
                let differs = channel > SAME;
                off += differs as usize;
                // A light gray copy of the second image, red where the two differ.
                let gray = (170 + (q[0] as u32 + q[1] as u32 + q[2] as u32) / 9) as u8;
                image.rgba[at..at + 4].copy_from_slice(&if differs {
                    [230, 30, 30, 255]
                } else {
                    [gray, gray, gray, 255]
                });
            }
            rows.push((sum, off, counted));
        }
        let band_height = height.div_ceil(bands).max(1);
        let bands = (0..height)
            .step_by(band_height)
            .map(|top| {
                let bottom = (top + band_height).min(height);
                let (sum, off, counted) = totals(&rows[top..bottom]);
                let pixels = counted.max(1) as f64;
                Band { top, bottom, mean: sum / pixels, share_off: off as f64 / pixels }
            })
            .collect();
        let (sum, off, counted) = totals(&rows);
        let pixels = counted.max(1) as f64;
        let left_out = 1.0 - counted as f64 / (width * height).max(1) as f64;
        Difference { mean: sum / pixels, share_off: off as f64 / pixels, left_out, bands, image }
    }
}

fn totals(rows: &[(f64, usize, usize)]) -> (f64, usize, usize) {
    rows.iter().fold((0.0, 0, 0), |(s, o, c), (sum, off, counted)| (s + sum, o + off, c + counted))
}

/// RGBA pixels, row after row.
struct Image {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Image {
    fn from_png(data: &[u8]) -> Result<Image> {
        let (width, height, rows) = png_read(data)?;
        Ok(Image { width, height, rgba: rows.concat() })
    }

    fn rows(&self) -> Vec<Vec<u8>> {
        self.rgba.chunks(self.width * 4).map(<[u8]>::to_vec).collect()
    }

    fn top(&self, height: usize) -> Image {
        Image { width: self.width, height, rgba: self.rgba[..self.width * height * 4].to_vec() }
    }

    /// Bilinear: each new pixel mixes the four nearest old ones. Enough for the small factors between a reference and
    /// a screen, or between two phones' densities.
    fn resized(&self, width: usize, height: usize) -> Image {
        if (width, height) == (self.width, self.height) {
            return Image { width, height, rgba: self.rgba.clone() };
        }
        let (sx, sy) = (self.width as f64 / width as f64, self.height as f64 / height as f64);
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            let fy = ((y as f64 + 0.5) * sy - 0.5).clamp(0.0, (self.height - 1) as f64);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(self.height - 1);
            for x in 0..width {
                let fx = ((x as f64 + 0.5) * sx - 0.5).clamp(0.0, (self.width - 1) as f64);
                let (x0, tx) = (fx.floor() as usize, fx.fract());
                let x1 = (x0 + 1).min(self.width - 1);
                for c in 0..4 {
                    let at = |x: usize, y: usize| self.rgba[(y * self.width + x) * 4 + c] as f64;
                    let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                    let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                    rgba.push((top * (1.0 - ty) + bottom * ty).round() as u8);
                }
            }
        }
        Image { width, height, rgba }
    }

    /// Images next to each other with a white gap, top edges together.
    fn side_by_side(images: &[&Image]) -> Image {
        const GAP: usize = 16;
        let height = images.iter().map(|i| i.height).max().unwrap_or(0);
        let width = images.iter().map(|i| i.width).sum::<usize>() + GAP * images.len().saturating_sub(1);
        let mut rgba = vec![255; width * height * 4];
        let mut left = 0;
        for image in images {
            for y in 0..image.height {
                let from = &image.rgba[y * image.width * 4..(y + 1) * image.width * 4];
                let at = (y * width + left) * 4;
                rgba[at..at + from.len()].copy_from_slice(from);
            }
            left += image.width + GAP;
        }
        Image { width, height, rgba }
    }

    /// Smaller by a whole factor until it is at most `max_width` wide.
    fn fit(self, max_width: usize) -> Image {
        let factor = self.width.div_ceil(max_width.max(1));
        if factor <= 1 {
            return self;
        }
        self.resized(self.width / factor, self.height / factor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(width: usize, height: usize, color: [u8; 4]) -> Image {
        Image { width, height, rgba: color.repeat(width * height) }
    }

    #[test]
    fn the_same_image_differs_by_nothing() {
        let image = filled(10, 8, [10, 20, 30, 255]);
        let difference = Difference::of(&image, &image, 4, &[]);
        assert_eq!((difference.mean, difference.share_off), (0.0, 0.0));
        assert_eq!(difference.bands.len(), 4);
    }

    #[test]
    fn a_changed_band_shows_in_that_band_only() {
        let a = filled(10, 8, [0, 0, 0, 255]);
        let mut b = filled(10, 8, [0, 0, 0, 255]);
        b.rgba[..10 * 2 * 4].copy_from_slice(&[255, 0, 0, 255].repeat(20)); // the top two rows red
        let difference = Difference::of(&a, &b, 4, &[]);
        assert_eq!(difference.share_off, 0.25);
        assert_eq!(difference.bands[0].share_off, 1.0);
        assert!(difference.bands[1..].iter().all(|band| band.share_off == 0.0));
        // The changed rows masked: nothing differs, and a quarter of the pixels is left out.
        let masked = Difference::of(&a, &b, 4, &[[0.0, 0.0, 10.0, 2.0]]);
        assert_eq!((masked.share_off, masked.left_out), (0.0, 0.25));
        assert_eq!(masked.bands[0].share_off, 0.0);
    }

    #[test]
    fn an_ignored_part_maps_into_the_resized_image() {
        let piece = Piece {
            image: filled(1, 1, [0; 4]),
            frame: [100.0, 50.0, 200.0, 100.0],
            ignored: vec![[150.0, 75.0, 50.0, 25.0]],
        };
        assert_eq!(piece.mask(400, 200), vec![[100.0, 50.0, 100.0, 50.0]]);
    }

    #[test]
    fn resizing_keeps_a_flat_color_and_the_size_asked() {
        let image = filled(6, 4, [40, 80, 120, 255]).resized(9, 5);
        assert_eq!((image.width, image.height), (9, 5));
        assert!(image.rgba.chunks(4).all(|p| p == [40, 80, 120, 255]));
        let small = filled(4000, 10, [1, 2, 3, 255]).fit(1800);
        assert_eq!(small.width, 1333);
    }
}
