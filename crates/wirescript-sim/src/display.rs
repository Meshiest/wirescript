//! `@display` screens as the simulator sees them: the text each row's Text
//! Decal holds, decoded back into pixels.
//!
//! A row is the text the display's rebuild writes: one `█` per pixel, a
//! `<color="RRGGBB">` tag (or the three-digit `<color="RGB">`) where the
//! color changes, and a space for a transparent pixel. A tag needs no close
//! and holds until the next one.

use crate::graph::GateId;

/// One pixel: its sRGB color, or `None` when transparent.
pub type Pixel = Option<[u8; 3]>;

/// A display as the program declared it, and the Text Decal of each row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayMeta {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// The row gates, top row first; `None` for a row the save lacks.
    pub rows: Vec<Option<GateId>>,
}

/// One display's current picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Each row's text, top row first.
    pub rows: Vec<String>,
}

impl Screen {
    fn new(name: &str, width: u32, height: u32) -> Self {
        Self {
            name: name.to_string(),
            width,
            height,
            rows: vec![String::new(); height as usize],
        }
    }

    /// Every pixel, row-major.
    pub fn pixels(&self) -> Vec<Pixel> {
        self.rows
            .iter()
            .flat_map(|r| decode_row(r, self.width as usize))
            .collect()
    }

    /// The pixel at `(x, y)`, `None` outside the display too.
    pub fn pixel(&self, x: u32, y: u32) -> Pixel {
        if x >= self.width {
            return None;
        }
        let row = self.rows.get(y as usize)?;
        decode_row(row, self.width as usize)[x as usize]
    }

    /// The picture as plain text: a header, then one line per row with a
    /// pixel as `RRGGBB` or `......` when transparent.
    pub fn render_plain(&self, tick: u64) -> Vec<String> {
        let mut out = vec![format!("[display {} @{tick}]", self.name)];
        for row in &self.rows {
            let cells: Vec<String> = decode_row(row, self.width as usize)
                .into_iter()
                .map(|p| match p {
                    Some([r, g, b]) => format!("{r:02X}{g:02X}{b:02X}"),
                    None => "......".to_string(),
                })
                .collect();
            out.push(cells.join(" "));
        }
        out
    }

    /// The picture for a terminal: a `██` per pixel in its 24-bit color, a
    /// blank pair for a transparent one.
    pub fn render_ansi(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| {
                let mut line = String::new();
                for p in decode_row(row, self.width as usize) {
                    match p {
                        Some([r, g, b]) => line += &format!("\x1b[38;2;{r};{g};{b}m██"),
                        None => line += "\x1b[0m  ",
                    }
                }
                line += "\x1b[0m";
                line
            })
            .collect()
    }
}

/// The color a `<color="...">` tag's value names: six or three hex digits.
fn tag_color(hex: &str) -> Option<[u8; 3]> {
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    match digits.as_slice() {
        [r1, r2, g1, g2, b1, b2] => Some([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2]),
        [r, g, b] => Some([r * 17, g * 17, b * 17]),
        _ => None,
    }
}

/// The `width` pixels a row's text draws. A `█` before any tag draws white,
/// the Text Decal's own color; a row shorter than `width` ends transparent.
pub fn decode_row(text: &str, width: usize) -> Vec<Pixel> {
    let mut out = Vec::with_capacity(width);
    let mut color = [255, 255, 255];
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if out.len() == width {
            break;
        }
        if let Some(tag) = rest.strip_prefix("<color=\"")
            && let Some((hex, after)) = tag.split_once("\">")
        {
            if let Some(c) = tag_color(hex) {
                color = c;
            }
            rest = after;
            continue;
        }
        match c {
            '█' => out.push(Some(color)),
            ' ' => out.push(None),
            _ => {}
        }
        rest = &rest[c.len_utf8()..];
    }
    out.resize(width, None);
    out
}

/// A picture a display showed: the tick it changed on and its rows then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub tick: u64,
    pub screen: Screen,
}

/// The displays of a run: each one's current picture, and every picture it
/// has shown.
#[derive(Clone, Debug, Default)]
pub struct Displays {
    screens: Vec<Screen>,
    dirty: Vec<bool>,
    frames: Vec<Frame>,
    /// Keep no frames, only which displays changed since the last
    /// [`Displays::take_changed`]: a live run has no use for the history.
    pub live: bool,
    changed: Vec<usize>,
}

impl Displays {
    /// Display `name` exists, `width` x `height`, every pixel transparent.
    pub fn add(&mut self, name: &str, width: u32, height: u32) {
        if self.get(name).is_none() {
            self.screens.push(Screen::new(name, width, height));
            self.dirty.push(false);
        }
    }

    /// Row `row` of display `name` now reads `text`.
    pub fn set_row(&mut self, name: &str, row: u32, text: &str) {
        let Some(i) = self.screens.iter().position(|s| s.name == name) else {
            return;
        };
        if let Some(r) = self.screens[i].rows.get_mut(row as usize)
            && r != text
        {
            text.clone_into(r);
            self.dirty[i] = true;
        }
    }

    /// Tick `tick` has settled: each display that changed during it adds a
    /// frame.
    pub fn settle(&mut self, tick: u64) {
        for (i, (s, dirty)) in self.screens.iter().zip(&mut self.dirty).enumerate() {
            if !std::mem::take(dirty) {
                continue;
            }
            if !self.live {
                self.frames.push(Frame {
                    tick,
                    screen: s.clone(),
                });
            } else if !self.changed.contains(&i) {
                self.changed.push(i);
            }
        }
    }

    /// The displays a [`Displays::live`] run changed since the last call.
    pub fn take_changed(&mut self) -> Vec<&Screen> {
        let changed = std::mem::take(&mut self.changed);
        changed
            .into_iter()
            .filter_map(|i| self.screens.get(i))
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&Screen> {
        self.screens.iter().find(|s| s.name == name)
    }

    pub fn screens(&self) -> &[Screen] {
        &self.screens
    }

    /// Every picture shown, in tick order.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }
}

#[cfg(test)]
mod tests;
