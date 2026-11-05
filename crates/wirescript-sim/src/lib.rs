//! A headless simulator for Brickadia logic gates.
//!
//! It runs a save's wire graph tick by tick, as the game schedules it: the
//! rules are the in-game measurements in the simulator project's
//! `ENGINE_MODEL.md`, and gate values come from the certified evaluator
//! ([`wirescript_ir::semantics`]). A gate the evaluator does not certify, or a
//! world gate the [`World`] does not script, is an error naming the gate, never
//! a guess.
//!
//! A Wirescript program runs as the save it compiles to ([`Sim::from_program`]),
//! so fan-out follows stored wire order and chips run as separate grids; any
//! `.brz` runs through the same loader ([`Sim::from_brz`]).

pub mod assumed;
pub mod graph;
pub mod load;
pub mod machine;
pub mod ports;
pub mod program;
#[cfg(feature = "compile")]
pub mod runner;
pub mod script_world;
pub mod value;
pub mod world;

pub use wirescript_ir::collections;

use std::fmt;

use wirescript_ir::{EmitOptions, Program, SourceRange};

pub use graph::{GateId, Graph};
pub use machine::Machine;
pub use value::Value;
pub use script_world::ScriptWorld;
pub use world::{ChatLine, NullWorld, World, WorldEvent};

/// The frame time each tick advances timers by.
#[derive(Clone, Debug)]
pub enum Dt {
    /// The same `dt` every tick.
    Constant(f64),
    /// `dt` per tick from tick 0; the last entry repeats.
    Schedule(Vec<f64>),
}

#[derive(Clone, Debug)]
pub struct SimOptions {
    /// `None` runs units in save order; `Some(seed)` runs a seeded
    /// permutation, for the orders the game leaves open.
    pub seed: Option<u64>,
    pub dt: Dt,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            seed: None,
            dt: Dt::Constant(1.0 / 60.0),
        }
    }
}

impl SimOptions {
    pub fn dt_at(&self, tick: u64) -> f64 {
        match &self.dt {
            Dt::Constant(dt) => *dt,
            Dt::Schedule(v) => v
                .get(tick as usize)
                .or(v.last())
                .copied()
                .unwrap_or(1.0 / 60.0),
        }
    }
}

/// A simulation that cannot go on: a gate it does not model or certify, or a
/// malformed graph.
#[derive(Clone, Debug)]
pub struct SimError {
    pub tick: u64,
    pub gate: Option<GateId>,
    /// The gate, as `Class at line:col` when the source is known.
    pub at: String,
    pub span: Option<SourceRange>,
    pub message: String,
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tick {}: {}: {}", self.tick, self.at, self.message)
    }
}

impl std::error::Error for SimError {}

impl SimError {
    fn load(message: String) -> Self {
        Self {
            tick: 0,
            gate: None,
            at: "load".into(),
            span: None,
            message,
        }
    }
}

/// A loaded program, ready to run.
pub struct Sim {
    pub machine: Machine,
}

/// A loaded graph that can start any number of runs, each with its own world
/// and options.
pub struct Loaded {
    pub graph: Graph,
}

impl Loaded {
    pub fn from_brz(bytes: &[u8]) -> Result<Self, SimError> {
        let save = load::read_brz(bytes).map_err(|e| SimError::load(e.to_string()))?;
        let graph = Graph::build(save).map_err(|e| SimError::load(e.to_string()))?;
        Ok(Self { graph })
    }

    pub fn from_program(program: &Program, emit: &EmitOptions) -> Result<Self, SimError> {
        let graph = program::graph_from_program(program, emit)
            .map_err(|e| SimError::load(e.to_string()))?;
        Ok(Self { graph })
    }

    /// `world`, already assembled from `program`, with `node_bricks` the
    /// brick each node became.
    pub fn from_world(
        world: &brdb::World,
        node_bricks: &collections::HashMap<wirescript_ir::NodeId, usize>,
        program: &Program,
    ) -> Result<Self, SimError> {
        let graph = program::graph_from_world(world, node_bricks, program)
            .map_err(|e| SimError::load(e.to_string()))?;
        Ok(Self { graph })
    }

    pub fn start(&self, world: Box<dyn World>, opts: SimOptions) -> Sim {
        Sim {
            machine: Machine::new(self.graph.clone(), world, opts),
        }
    }

    /// [`Loaded::start`] for a single run, taking the graph rather than
    /// copying it.
    pub fn into_sim(self, world: Box<dyn World>, opts: SimOptions) -> Sim {
        Sim {
            machine: Machine::new(self.graph, world, opts),
        }
    }
}

impl Sim {
    /// The `.brz` save in `bytes`.
    pub fn from_brz(bytes: &[u8], world: Box<dyn World>, opts: SimOptions) -> Result<Self, SimError> {
        let Loaded { graph } = Loaded::from_brz(bytes)?;
        Ok(Self {
            machine: Machine::new(graph, world, opts),
        })
    }

    /// `program`, as the save `emit` would write for it.
    pub fn from_program(
        program: &Program,
        emit: &EmitOptions,
        world: Box<dyn World>,
        opts: SimOptions,
    ) -> Result<Self, SimError> {
        let Loaded { graph } = Loaded::from_program(program, emit)?;
        Ok(Self {
            machine: Machine::new(graph, world, opts),
        })
    }

    /// Run through tick `last` (tick 0 is the Load Event).
    pub fn run_until(&mut self, last: u64) -> Result<(), SimError> {
        self.machine.run_until(last)
    }

    /// Every chat line printed so far.
    pub fn chat(&self) -> &[ChatLine] {
        self.machine.world.chat_lines()
    }

    /// Queue `line`, a script line without its tick, for the next tick.
    pub fn push(&mut self, line: &str) -> Result<(), String> {
        let tick = self.machine.tick() + 1;
        self.machine.world.push_line(tick, line)
    }
}
