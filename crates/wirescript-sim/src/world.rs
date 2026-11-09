//! The world a program runs in: everything outside the logic gates.
//!
//! The simulator models logic exactly and leaves the world to a [`World`].
//! [`NullWorld`] records chat and refuses every world gate. A scripted world
//! (players, chat commands, button presses) implements the same trait.

use crate::display::Displays;
use crate::graph::{Gate, GateId};
use crate::value::Value;

/// One line a program printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatLine {
    pub tick: u64,
    pub text: String,
}

/// What a world gate needs from the simulator: the gate, and its inputs read
/// on demand.
pub trait GateInputs {
    fn gate(&self) -> &Gate;

    /// The value on input `name`, `None` when unwired.
    fn input(&mut self, name: &str) -> Result<Option<Value>, crate::SimError>;
}

/// The result of running a world gate: values for its outputs, and the exec
/// output to fire next (usually `ExecOut`).
#[derive(Default)]
pub struct WorldResult {
    pub outputs: Vec<(String, Value)>,
    pub fire: Option<String>,
    /// Arrays the gate fills: the input that references the array
    /// (`ArrayVarRef`) and the elements it holds afterwards, replacing what
    /// it held.
    pub arrays: Vec<(String, Vec<Value>)>,
}

pub trait World {
    /// `BroadcastChatMessage` printed `text` on `tick`.
    fn chat(&mut self, tick: u64, text: &str);

    /// Run world gate `gate`. The default refuses it: a world the test does
    /// not script is an error, not a guess.
    fn exec_gate(
        &mut self,
        tick: u64,
        id: GateId,
        gate: &mut dyn GateInputs,
    ) -> Result<WorldResult, String> {
        let _ = (tick, id);
        Err(format!(
            "{} is a world gate the simulator does not model",
            gate.gate().class
        ))
    }

    /// The outputs of a component whose values come from the world (a seat
    /// control splitter), by name, read again every tick. The default
    /// refuses it, as `exec_gate` does.
    fn component_outputs(
        &mut self,
        tick: u64,
        id: GateId,
        gate: &mut dyn GateInputs,
    ) -> Result<Vec<(String, Value)>, String> {
        let _ = (tick, id);
        Err(format!(
            "{} reads the world, which this run does not script",
            gate.gate().class
        ))
    }

    /// Queue `line`, a script line without its tick, to happen on `tick`
    /// (an interactive session). The default takes none.
    fn push_line(&mut self, tick: u64, line: &str) -> Result<(), String> {
        let _ = (tick, line);
        Err("this run has no scripted world".to_string())
    }

    /// The texts player `name`'s screen shows, by text id.
    fn screen(&self, name: &str) -> Option<Vec<(i64, String)>> {
        let _ = name;
        None
    }

    /// Every line printed so far.
    fn chat_lines(&self) -> &[ChatLine];

    /// The lines printed since the last call, forgotten here: a live run
    /// keeps no history. A world without chat history gives none.
    fn take_chat(&mut self) -> Vec<ChatLine> {
        Vec::new()
    }

    /// The events the world raises on `tick`, delivered ahead of the tick's
    /// handlers.
    fn events(&mut self, tick: u64) -> Vec<WorldEvent> {
        let _ = tick;
        Vec::new()
    }

    /// Something the world was told to do that has not happened yet.
    fn unfinished(&self) -> Option<String> {
        None
    }

    /// The displays of the run, when this world keeps them.
    fn displays(&self) -> Option<&Displays> {
        None
    }

    /// The displays of the run, to record into.
    fn displays_mut(&mut self) -> Option<&mut Displays> {
        None
    }

    /// Who sits in each `@seat`, by seat name, when this world seats players.
    fn seated(&self) -> Option<&std::collections::BTreeMap<String, String>> {
        None
    }

    /// Tick `tick` has run: frame each display that changed during it.
    fn settle_displays(&mut self, tick: u64) {
        if let Some(d) = self.displays_mut() {
            d.settle(tick);
        }
    }
}

/// A seat no `sit` line has filled: empty, with no character in it.
pub fn empty_seat() -> Vec<(String, Value)> {
    vec![
        ("bIsOccupied".into(), Value::Bool(false)),
        ("SittingCharacter".into(), Value::Object(0)),
    ]
}

/// Whether a world-value component is a seat.
pub fn is_seat(class: &str) -> bool {
    class.ends_with("Internal_Seat")
}

/// An event the world raises: every event gate of `class` (and, with
/// `filter`, whose config field holds that text) fires with these outputs.
#[derive(Clone, Debug, Default)]
pub struct WorldEvent {
    pub class: String,
    pub filter: Option<(String, String)>,
    pub outputs: Vec<(String, Value)>,
    /// The one gate the event is for (a message box being answered), in place
    /// of every event gate of `class`.
    pub gate: Option<GateId>,
    /// The exec output that fires; `ExecOut` when `None`.
    pub fire: Option<String>,
}

/// A world with no players and no physics: prints are recorded, every world
/// gate is an error.
#[derive(Default)]
pub struct NullWorld {
    pub lines: Vec<ChatLine>,
    pub displays: Displays,
}

impl World for NullWorld {
    fn chat(&mut self, tick: u64, text: &str) {
        self.lines.push(ChatLine {
            tick,
            text: text.to_string(),
        });
    }

    fn chat_lines(&self) -> &[ChatLine] {
        &self.lines
    }

    fn displays(&self) -> Option<&Displays> {
        Some(&self.displays)
    }

    fn displays_mut(&mut self) -> Option<&mut Displays> {
        Some(&mut self.displays)
    }
}
