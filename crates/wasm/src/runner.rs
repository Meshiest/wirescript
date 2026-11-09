//! The interactive runner: one program run tick by tick, driven by the page
//! (a seat taken, keys held, as world script lines) and read back as its
//! displays, seats and chat.

use serde::Serialize;
use wasm_bindgen::prelude::*;
use wirescript::emit::EmitOptions;
use wirescript::exterior::{self, DisplayInfo};
use wirescript::ir::{Literal, Module};
use wirescript_sim::{Dt, Loaded, ScriptWorld, Sim, SimOptions};

/// A display as the page lays it out.
#[derive(Serialize)]
struct DisplayOut {
    name: String,
    width: u32,
    height: u32,
}

/// A seat as the page lays it out: its side of the chip, in row order.
#[derive(Serialize)]
struct SeatOut {
    name: String,
    side: &'static str,
    label: String,
    /// `#rrggbb`, or `null` for the default.
    color: Option<String>,
}

#[derive(Serialize)]
struct Layout {
    displays: Vec<DisplayOut>,
    seats: Vec<SeatOut>,
}

#[derive(Serialize)]
struct Line {
    tick: u64,
    text: String,
}

/// One row of a display: one entry per pixel, `0xRRGGBB` or `-1` when
/// transparent.
#[derive(Serialize)]
struct Row {
    y: usize,
    pixels: Vec<i32>,
}

/// The rows of a display that changed.
#[derive(Serialize)]
struct Picture {
    name: String,
    rows: Vec<Row>,
}

/// What changed since the last step.
#[derive(Serialize)]
struct Update {
    tick: u64,
    chat: Vec<Line>,
    /// Each display some row of which changed.
    displays: Vec<Picture>,
    /// Who sits in each seat, by seat name.
    seated: std::collections::BTreeMap<String, String>,
    error: Option<String>,
}

/// The most chat lines one update carries; the rest are counted in a line of
/// their own.
const CHAT_PER_UPDATE: usize = 50;
/// The most characters of a chat line an update carries.
const CHAT_LINE_CHARS: usize = 2000;

fn layout_of(module: &Module) -> Layout {
    let mut displays = Vec::new();
    let mut seats = Vec::new();
    for (_, node) in module.nodes_in_order() {
        if let Some(d) = DisplayInfo::of(node) {
            displays.push(DisplayOut {
                name: d.name,
                width: d.width,
                height: d.height,
            });
        } else if let Some((name, side)) = exterior::seat_of(node) {
            let label = match node
                .properties
                .get(&wirescript::intern::intern("PromptCustomLabel"))
            {
                Some(Literal::String(s)) if !s.is_empty() => s.clone(),
                _ => name.clone(),
            };
            let color = match node.properties.get(&*wirescript::intern::sym::BRICK_COLOR) {
                Some(Literal::Color { r, g, b, .. }) => Some(format!("#{r:02x}{g:02x}{b:02x}")),
                _ => None,
            };
            seats.push(SeatOut {
                name,
                side: side.as_str(),
                label,
                color,
            });
        }
    }
    Layout { displays, seats }
}

/// One program running in the page.
#[wasm_bindgen]
pub struct Runner {
    sim: Sim,
    layout: String,
    started: bool,
    /// The text of each display row the page last got, by display name.
    sent: wirescript::collections::HashMap<String, Vec<String>>,
    error: Option<String>,
}

#[wasm_bindgen]
impl Runner {
    /// Compile `source` and load it, ready for its first tick. Throws the
    /// compile error as a string.
    #[wasm_bindgen(constructor)]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "a wasm_bindgen export keeps its JS-facing signature"
    )]
    pub fn new(
        source: String,
        module_name: Option<String>,
        files_json: Option<String>,
        prefabs_json: Option<String>,
    ) -> Result<Runner, JsValue> {
        let file = module_name.as_deref().unwrap_or("inline");
        let files_json = files_json.unwrap_or_else(|| "{}".into());
        let emit: EmitOptions = crate::sim_emit_options(file, &files_json, prefabs_json.as_deref());
        let loader = crate::make_loader(&files_json);
        let input = wirescript::CompileInput::new(&source, file);
        let result = wirescript::compile_to_world_with_loader(input, emit, &loader)
            .map_err(|e| JsValue::from_str(&format!("compile: {e}")))?;
        let layout = serde_json::to_string(&layout_of(&result.program.module))
            .unwrap_or_else(|_| "{}".into());
        let loaded = Loaded::from_world(&result.world, &result.node_bricks, &result.program)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let opts = SimOptions {
            seed: None,
            dt: Dt::Constant(1.0 / 60.0),
        };
        // A live run keeps no history of pictures or chat: each step hands
        // over what is new and forgets it.
        let mut world = ScriptWorld::default();
        world.displays.live = true;
        Ok(Self {
            sim: loaded.into_sim(Box::new(world), opts),
            layout,
            started: false,
            sent: wirescript::collections::HashMap::default(),
            error: None,
        })
    }

    /// The displays and seats as JSON `{ displays: [{ name, width, height
    /// }], seats: [{ name, side, label, color }] }`, seats in each side's row
    /// order.
    pub fn layout(&self) -> String {
        self.layout.clone()
    }

    /// Run `ticks` more ticks (the first call also runs tick 0) and return
    /// what changed as JSON `{ tick, chat: [{ tick, text }], displays: [{
    /// name, rows: [{ y, pixels }] }], seated: { seat: player }, error }`,
    /// with only the display rows that changed. After an error the run stops
    /// where it was.
    pub fn step(&mut self, ticks: u32) -> String {
        if self.error.is_none() {
            let target = if self.started {
                self.sim.machine.tick() + u64::from(ticks)
            } else {
                u64::from(ticks.saturating_sub(1))
            };
            self.started = true;
            if let Err(e) = self.sim.run_until(target) {
                self.error = Some(e.to_string());
            }
        }
        let world = &mut self.sim.machine.world;
        let said = world.take_chat();
        let more = said.len().saturating_sub(CHAT_PER_UPDATE);
        let mut chat: Vec<Line> = said
            .into_iter()
            .take(CHAT_PER_UPDATE)
            .map(|l| Line {
                tick: l.tick,
                text: l.text.chars().take(CHAT_LINE_CHARS).collect(),
            })
            .collect();
        if more > 0 {
            chat.push(Line {
                tick: self.sim.machine.tick(),
                text: format!("({more} more lines this tick)"),
            });
        }
        let world = &mut self.sim.machine.world;
        let mut displays = Vec::new();
        if let Some(d) = world.displays_mut() {
            for s in d.take_changed() {
                let sent = self.sent.entry(s.name.clone()).or_default();
                sent.resize(s.rows.len(), String::new());
                let mut rows = Vec::new();
                // The page starts every row transparent, as a blank row draws.
                for (y, text) in s.rows.iter().enumerate() {
                    if sent[y] == *text {
                        continue;
                    }
                    text.clone_into(&mut sent[y]);
                    let pixels = wirescript_sim::display::decode_row(text, s.width as usize)
                        .into_iter()
                        .map(|p| p.map_or(-1, |[r, g, b]| i32::from_be_bytes([0, r, g, b])))
                        .collect();
                    rows.push(Row { y, pixels });
                }
                if !rows.is_empty() {
                    displays.push(Picture {
                        name: s.name.clone(),
                        rows,
                    });
                }
            }
        }
        let seated = world.seated().cloned().unwrap_or_default();
        let update = Update {
            tick: self.sim.machine.tick(),
            chat,
            displays,
            seated,
            error: self.error.clone(),
        };
        serde_json::to_string(&update).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    }

    /// Queue `line`, a world script line without its tick (`sit you p1`,
    /// `input you Forward 1`, `stand you`), for the next tick. Returns the
    /// problem with it, or an empty string.
    pub fn push(&mut self, line: &str) -> String {
        self.sim.push(line).err().unwrap_or_default()
    }
}
