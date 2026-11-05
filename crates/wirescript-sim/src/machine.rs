//! The scheduler: how a tick runs, as measured in game (`ENGINE_MODEL.md`
//! sections 9 to 11 in the simulator project).
//!
//! A *unit* is a gate the scheduler starts rather than an exec call: a gate
//! whose Exec pin a value drives, a change detector, an event, a clock, a
//! buffer of an exec signal. Each unit has a rank. A tick is one pass over
//! the pending units in rank order; a unit that becomes pending behind the
//! pass waits for the next tick, ahead of that tick's other work. The game
//! fixes no order between units fired by one change, so the ranks are a
//! seeded permutation (the default is save order).
//!
//! Running a unit runs its exec chain depth-first, fan-out in stored wire
//! order. An exec value output reaches its readers at once. A var write
//! changes storage at once but reaches the gates off the var only when the
//! chain is done. A call into another grid (a fork into a chip) runs when
//! the chain is done. Then var writes are published, pure gates settle, and
//! any Exec pin whose value changed to truthy makes its unit pending.

use std::collections::{BTreeMap, VecDeque};

use wirescript_ir::semantics;

use crate::collections::{HashMap, HashSet};
use crate::graph::{Detector, Gate, GateId, Graph, Op};
use crate::value::Value;
use crate::world::{GateInputs, World};
use crate::{SimError, SimOptions};

mod arrays;
mod callable;

use arrays::{ArrayStore, MapStore, array_from_field, map_from_field};
use callable::Callable;

/// Why a unit runs.
#[derive(Clone, Debug)]
enum Activation {
    /// A value driving Exec pin `input` changed to truthy.
    Pin(u16),
    /// The gate's own exec output fires (a change detector, the Load Event, a
    /// clock, a buffer of an exec signal).
    Fire,
    /// A custom event arrived with this data.
    Event(Vec<Option<Value>>),
    /// The world raised the gate's event with these outputs; it fires the
    /// named exec output, `ExecOut` when none is named.
    World(Vec<(String, Value)>, Option<String>),
    /// The gate fires exec output `o` (an edge detector's rising or falling
    /// side).
    Output(u16),
}

/// A buffer's timer, following the fields the save stores for it.
#[derive(Clone, Debug, Default)]
struct Buffer {
    /// Exec mode: the input is an exec signal, and each call fires the output
    /// later.
    exec_mode: bool,
    /// The value in flight and the ticks (or seconds) left before it lands.
    in_flight: Option<(Option<Value>, Countdown)>,
    /// Whether an input arrived while a value was in flight, and the latest
    /// one (the save's `bHasQueued` and `Buffered`).
    has_queued: bool,
    queued: Option<Value>,
    /// Exec mode: the ticks the pending fires land on.
    fires: VecDeque<Countdown>,
}

#[derive(Clone, Copy, Debug)]
enum Countdown {
    Ticks(i64),
    /// Seconds to wait and seconds accumulated so far, in the `f32` the save
    /// stores them in.
    Seconds { wait: f32, elapsed: f32 },
}

#[derive(Clone, Debug)]
enum State {
    None,
    Var(Option<Value>),
    Array(ArrayStore),
    Map(MapStore),
    Buffer(Box<Buffer>),
    Clock { elapsed: f64 },
    /// A queue's records, oldest first, each with the wait left before it is
    /// released.
    Queue(VecDeque<(Vec<Option<Value>>, Countdown)>),
    Stopwatch { time: f64, running: bool },
    Tween(Box<Tween>),
    /// A Dampen's velocity per component.
    Dampen([f64; 4]),
    /// A spawner's live copies, oldest first.
    Spawner(Vec<SpawnedCopy>),
}

/// One spawned copy of a prefab: its entity, its gates, and when its
/// lifetime ends (server uptime).
#[derive(Clone, Debug)]
struct SpawnedCopy {
    entity: u64,
    gates: std::ops::Range<GateId>,
    ends_at: Option<f64>,
}

/// Where spawned copies' entity ids start, clear of the world's ids.
const SPAWN_OBJECTS: u64 = 1 << 52;

/// The most gates spawning may grow a run to before it is called runaway.
const MAX_GATES: usize = 20_000_000;

/// A Tween's run from `start` to `target`, following its saved fields.
#[derive(Clone, Debug)]
struct Tween {
    start: Option<Value>,
    target: Option<Value>,
    elapsed: f64,
    running: bool,
}

/// The Unix time the simulation starts at (2026-01-01T00:00:00Z), so
/// `GetUnixEpoch` is the same every run.
const START_EPOCH: i64 = 1_767_225_600;

/// Where the objects asset references give start, clear of the world's ids.
const ASSET_OBJECTS: u64 = 1 << 48;

/// The most records a queue holds; a record past this is dropped.
const MAX_QUEUE_RECORDS: usize = 256;

/// One custom event send, delivered next tick.
struct Send {
    name: String,
    global: bool,
    data: Vec<Option<Value>>,
}

/// A small seeded generator for the choices the game leaves open.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in 0..1.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

pub struct Machine {
    pub graph: Graph,
    pub world: Box<dyn World>,
    opts: SimOptions,
    /// The current tick; the Load Event runs on tick 0.
    tick: u64,
    dt: f64,
    uptime: f64,
    started: bool,
    /// Each output's current value; `None` reads as unwired.
    outs: Vec<Vec<Option<Value>>>,
    /// Pure gates whose outputs are stale.
    dirty: Vec<bool>,
    state: Vec<State>,
    /// The last value each watched input saw.
    last: HashMap<(GateId, u16), Option<Value>>,
    to_check: VecDeque<(GateId, u16)>,
    queued_check: HashSet<(GateId, u16)>,
    /// Each unit's rank; `u32::MAX` for a gate that is never a unit.
    rank: Vec<u32>,
    /// The rank the next unit set up gets.
    next_rank: u32,
    pending: BTreeMap<u32, VecDeque<(GateId, Activation)>>,
    /// Work that lands ahead of a tick's handlers: units carried from the
    /// last tick, custom events, buffers of an exec signal.
    late: BTreeMap<u32, VecDeque<(GateId, Activation)>>,
    /// Custom events sent last tick: they land after the rest of the late
    /// work, change reactions and buffers included (P15 `CB1 R1`, P35a
    /// `C1 R1`, P16 `G R1`).
    late_events: VecDeque<(GateId, Activation)>,
    in_late: bool,
    /// Pure gates marked stale since the last settle.
    stale: Vec<GateId>,
    carried: Vec<(GateId, Activation)>,
    cursor: Option<u32>,
    forks: VecDeque<(GateId, u16)>,
    written: Vec<GateId>,
    written_set: HashSet<GateId>,
    /// Vars this chain wrote inside a callable, and the watched inputs their
    /// change reaches: those act a tick late whatever their rank (P27 to
    /// P29), where a plain chain's write follows rank (P04 `C1@5`). A plain
    /// write to the same var in the chain, before or after, makes it act on
    /// time (P30 `b2@4 b3@6`).
    written_in_plane: HashSet<GateId>,
    written_outside: HashSet<GateId>,
    late_checks: HashSet<(GateId, u16)>,
    marking_late: bool,
    outbox: Vec<(GateId, Send)>,
    receivers: HashMap<String, Vec<GateId>>,
    timed: Vec<GateId>,
    time_sources: Vec<GateId>,
    rng: Rng,
    /// Exec gates run this tick, for the runaway guard.
    steps: u64,
    /// Totals over the whole run, for `stats`.
    total_steps: u64,
    total_evals: u64,
    callables: HashMap<GateId, Callable>,
    /// Each certified gate's inputs in evaluator order (`None` for a port
    /// the gate does not have).
    eval_inputs: Vec<Vec<Option<u16>>>,
    /// Callable invocations run this tick.
    invocations: u64,
    /// Callables running inside one another.
    depth: u32,
    /// A Break ran inside the innermost running loop.
    break_requested: bool,
    /// The grid of each running callable's plane, innermost last.
    planes: Vec<u32>,
    /// The grid each chip grid sits in.
    grid_parent: HashMap<u32, u32>,
    /// The Exec Unions that have passed an activation, for the tick and then
    /// for each running invocation (P25, P26: a Union passes once per tick).
    unions_passed: Vec<HashSet<GateId>>,
    /// The Container Change Detectors, and those whose container was written
    /// this tick (they fire next tick).
    container_watchers: Vec<GateId>,
    container_fires: Vec<GateId>,
    /// Prefab copies spawned so far, for their entity ids.
    spawned: u64,
    /// Where each spawned copy's gates start, and how far its grids moved.
    copy_offsets: Vec<(GateId, u32)>,
    /// Detector outputs pulsed, with the tick each last went high. P20g: a
    /// pulse is still high in the next tick's handlers, so it clears at the
    /// start of the tick after that.
    pulses: HashMap<(GateId, u16), u64>,
}

/// The most exec gates one tick may run before the simulator calls it a
/// runaway loop.
const MAX_STEPS_PER_TICK: u64 = 50_000_000;

/// The most UTF-8 bytes a string holds, so text beyond ASCII fits fewer
/// characters. A gate that produces a longer string gives it cut to the last
/// whole character that fits. Assumed, from the game update that allows
/// 32,768-character strings.
pub(crate) const MAX_STRING_BYTES: usize = 32_768;

/// Cut `s` to at most [`MAX_STRING_BYTES`], on a character boundary.
pub(crate) fn cap_string(s: &mut String) {
    if s.len() > MAX_STRING_BYTES {
        let mut at = MAX_STRING_BYTES;
        while !s.is_char_boundary(at) {
            at -= 1;
        }
        s.truncate(at);
    }
}

/// The most elements an array may hold before the simulator calls the
/// program runaway.
pub(crate) const MAX_ARRAY_LEN: usize = 10_000_000;

impl Machine {
    pub fn new(graph: Graph, world: Box<dyn World>, opts: SimOptions) -> Self {
        let rng = Rng(opts.seed.unwrap_or(0) ^ 0x5EED_0FD5_EED5);
        let mut m = Self {
            graph,
            world,
            dt: opts.dt_at(0),
            opts,
            tick: 0,
            uptime: 0.0,
            started: false,
            outs: Vec::new(),
            dirty: Vec::new(),
            state: Vec::new(),
            last: HashMap::default(),
            to_check: VecDeque::new(),
            queued_check: HashSet::default(),
            rank: Vec::new(),
            next_rank: 0,
            pending: BTreeMap::new(),
            late: BTreeMap::new(),
            late_events: VecDeque::new(),
            in_late: false,
            stale: Vec::new(),
            carried: Vec::new(),
            cursor: None,
            forks: VecDeque::new(),
            written: Vec::new(),
            written_set: HashSet::default(),
            written_in_plane: HashSet::default(),
            written_outside: HashSet::default(),
            late_checks: HashSet::default(),
            marking_late: false,
            outbox: Vec::new(),
            receivers: HashMap::default(),
            timed: Vec::new(),
            time_sources: Vec::new(),
            rng,
            steps: 0,
            total_steps: 0,
            total_evals: 0,
            callables: HashMap::default(),
            eval_inputs: Vec::new(),
            invocations: 0,
            depth: 0,
            break_requested: false,
            planes: Vec::new(),
            grid_parent: HashMap::default(),
            unions_passed: vec![HashSet::default()],
            pulses: HashMap::default(),
            container_watchers: Vec::new(),
            container_fires: Vec::new(),
            spawned: 0,
            copy_offsets: Vec::new(),
        };
        m.add_gates(0);
        m.resolve_handles(0, 0);
        m
    }

    /// Set up gates `from..` (every gate of a new machine, or a spawned
    /// copy): outputs from their saved fields, storage, timers, the events
    /// they receive, and ranks after every unit already ranked.
    fn add_gates(&mut self, from: GateId) {
        let graph = std::mem::take(&mut self.graph.gates);
        for (i, g) in graph.iter().enumerate().skip(from as usize) {
            let id = i as GateId;
            self.outs.push(
                g.outputs
                    .iter()
                    .map(|o| g.field_value(&o.name).or_else(|| o.default.clone()))
                    .collect::<Vec<_>>(),
            );
            self.dirty.push(g.op.is_pure());
            self.state.push(match g.op {
                Op::Var => State::Var(g.field_value("Value")),
                Op::ArrayVar => State::Array(array_from_field(g.field("Value"))),
                Op::MapVar => State::Map(map_from_field(g.field("Value"))),
                Op::BufferTicks | Op::BufferSeconds => {
                    self.timed.push(id);
                    let exec_mode = g.output("Output").is_some_and(|o| g.outputs[o as usize].exec);
                    State::Buffer(Box::new(Buffer {
                        exec_mode,
                        ..Buffer::default()
                    }))
                }
                Op::Clock => {
                    self.timed.push(id);
                    State::Clock { elapsed: 0.0 }
                }
                Op::Queue { .. } => {
                    self.timed.push(id);
                    State::Queue(VecDeque::new())
                }
                Op::Stopwatch => {
                    self.timed.push(id);
                    State::Stopwatch {
                        time: g.field_f64("Time").unwrap_or(0.0),
                        running: g.field_bool("bRunning").unwrap_or(false),
                    }
                }
                Op::Tween => {
                    self.timed.push(id);
                    State::Tween(Box::new(Tween {
                        start: g.field_value("StartValue"),
                        target: g.field_value("Target"),
                        elapsed: g.field_f64("Elapsed").unwrap_or(0.0),
                        running: g.field_bool("bRunning").unwrap_or(false),
                    }))
                }
                Op::Dampen => {
                    self.timed.push(id);
                    State::Dampen([0.0; 4])
                }
                Op::PrefabSpawner => {
                    self.timed.push(id);
                    State::Spawner(Vec::new())
                }
                _ => State::None,
            });
            // The saved state these gates show before anything runs.
            let saved = match g.op {
                Op::Toggle => Some(("Output", Value::Bool(g.field_bool("Value").unwrap_or(false)))),
                Op::Cycle => Some(("Output", Value::Int(g.field_i64("Value").unwrap_or(0)))),
                Op::Stopwatch => Some(("Time", Value::Float(g.field_f64("Time").unwrap_or(0.0)))),
                _ => None,
            };
            if let Some((name, v)) = saved
                && let Some(o) = g.output(name)
            {
                self.outs[i][o as usize] = Some(v);
            }
            if matches!(g.op, Op::DeltaTime | Op::ServerUptime | Op::UnixEpoch | Op::WorldValue) {
                self.time_sources.push(id);
            }
            if let Op::EventCustom { .. } = g.op
                && let Some(name) = g.field_str("EventName")
            {
                self.receivers.entry(name).or_default().push(id);
            }
            if g.op == Op::ContainerWatch {
                self.container_watchers.push(id);
            }
            self.eval_inputs.push(if g.op == Op::Eval {
                crate::ports::eval_ports(g).iter().map(|p| g.input(p)).collect()
            } else {
                Vec::new()
            });
        }
        self.graph.gates = graph;
        let n = self.graph.gates.len() as GateId;
        let mut units: Vec<GateId> = (from..n).filter(|&g| is_unit(&self.graph, g)).collect();
        let first = network_firsts(&self.graph, from);
        units.sort_by_key(|&g| (first[(g - from) as usize], self.graph.gate(g).addr, g));
        if self.opts.seed.is_some() {
            self.rng.shuffle(&mut units);
        }
        self.rank.resize(n as usize, u32::MAX);
        for &g in &units {
            self.rank[g as usize] = self.next_rank;
            self.next_rank += 1;
        }
        self.callables = callable::callables(&self.graph);
        self.grid_parent = self.graph.chip_grids.iter().map(|(addr, &grid)| (grid, addr.grid)).collect();
    }

    /// Resolve every saved component handle (a baked reference) of gates
    /// `from..` to the gate it names; a spawned copy's grids moved by
    /// `grid_offset`.
    fn resolve_handles(&mut self, from: GateId, grid_offset: u32) {
        for g in from as usize..self.graph.gates.len() {
            for i in 0..self.graph.gates[g].inputs.len() {
                let v = self.graph.gates[g].inputs[i].stored.take();
                self.graph.gates[g].inputs[i].stored = self.resolve(v, grid_offset);
            }
            for o in 0..self.outs[g].len() {
                let v = self.outs[g][o].take();
                self.outs[g][o] = self.resolve(v, grid_offset);
            }
            let state = std::mem::replace(&mut self.state[g], State::None);
            self.state[g] = match state {
                State::Var(v) => State::Var(self.resolve(v, grid_offset)),
                State::Array(mut a) => {
                    a.items = a.items.into_iter().filter_map(|v| self.resolve(Some(v), grid_offset)).collect();
                    State::Array(a)
                }
                other => other,
            };
        }
    }

    /// How far the grids of the copy gate `g` belongs to moved (0 outside
    /// any spawned copy).
    fn grid_offset_of(&self, g: GateId) -> u32 {
        self.copy_offsets.iter().rev().find(|&&(start, _)| start <= g).map_or(0, |&(_, o)| o)
    }

    /// `v` with a saved component handle turned into a reference to its gate.
    fn resolve(&self, v: Option<Value>, grid_offset: u32) -> Option<Value> {
        match v {
            Some(Value::Handle(h)) => {
                let c = h.brick.chunk_index;
                let addr = crate::load::BrickAddr::new(
                    h.brick.grid_persistent_index + grid_offset,
                    c,
                    h.brick.brick_index_in_chunk,
                );
                Some(match self.graph.by_addr.get(&(addr, h.component_type_index)) {
                    Some(&g) => Value::Ref(g),
                    None => Value::Handle(h),
                })
            }
            other => other,
        }
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Exec gates run and pure gates evaluated so far.
    pub fn stats(&self) -> (u64, u64) {
        (self.total_steps, self.total_evals)
    }

    /// Work that would still run on a later tick: events in flight, handlers
    /// waiting for their turn, buffered exec signals. Value buffers and clocks
    /// do not count; a program can keep those going forever.
    pub fn pending_work(&self) -> Option<&'static str> {
        if !self.outbox.is_empty() {
            return Some("custom events are still in flight");
        }
        if !self.carried.is_empty() || !self.late.is_empty() || !self.pending.is_empty() {
            return Some("handlers are still waiting to run");
        }
        let exec_buffered = self.state.iter().any(|s| match s {
            State::Buffer(b) => b.exec_mode && !b.fires.is_empty(),
            State::Queue(q) => !q.is_empty(),
            _ => false,
        });
        exec_buffered.then_some("buffered exec signals are still waiting to land")
    }

    /// What gate `g` produces from its current inputs, outside any tick: a
    /// pure gate's first output, or `"A"`/`"B"` for the side a branch takes.
    pub fn evaluate_alone(&mut self, g: GateId) -> Result<Option<Value>, SimError> {
        let gate = self.graph.gate(g);
        if gate.op == Op::Branch {
            let cond = self.read_named(g, "bCond")?;
            let side = if cond.as_ref().is_some_and(Value::truthy) { "A" } else { "B" };
            return Ok(Some(Value::Str(side.to_string())));
        }
        if !gate.op.is_pure() {
            return Err(self.error(g, "not a pure gate"));
        }
        self.invalidate(g);
        let out = value_output(self.graph.gate(g));
        match self.read_output(g, out)? {
            Some(Value::Poison(p)) => Err((*p.0).clone()),
            v => Ok(v),
        }
    }

    /// Run until tick `last` has finished (tick 0 is the Load Event).
    pub fn run_until(&mut self, last: u64) -> Result<(), SimError> {
        if !self.started {
            self.start()?;
        }
        while self.tick < last {
            self.step()?;
        }
        Ok(())
    }

    /// Tick 0: settle every gate from its stored state without firing
    /// anything, then run the Load Event.
    fn start(&mut self) -> Result<(), SimError> {
        self.started = true;
        self.steps = 0;
        self.settle_new(0)?;
        self.settle()?;
        self.fire_loaded(0);
        self.advance_clocks()?;
        self.pass()
    }

    /// Settle gates `from..` from their stored state without firing
    /// anything: compute the pure gates and note what each watched input
    /// holds.
    fn settle_new(&mut self, from: GateId) -> Result<(), SimError> {
        let n = self.graph.gates.len() as GateId;
        for g in from..n {
            if self.dirty[g as usize] {
                self.refresh(g)?;
            }
        }
        for g in from..n {
            for i in 0..self.graph.gate(g).inputs.len() as u16 {
                if self.is_watched(g, i) {
                    let v = self.read_input(g, i)?;
                    let gate = self.graph.gate(g);
                    if matches!(gate.op, Op::BufferTicks | Op::BufferSeconds)
                        && gate.inputs[i as usize].name == "Input"
                    {
                        let stored = gate.inputs[i as usize].stored.clone();
                        self.last.insert((g, i), stored);
                        self.watch(g, i);
                    } else if gate.op == Op::Tween {
                        // Compared with the saved target, so a different first
                        // target starts a run at load.
                        let saved = gate.field_value("Target");
                        self.last.insert((g, i), saved);
                        self.watch(g, i);
                    } else if let Some(saved) = seeded_last(gate) {
                        self.last.insert((g, i), Some(saved));
                        self.watch(g, i);
                    } else {
                        self.last.insert((g, i), v);
                    }
                }
            }
        }
        Ok(())
    }

    /// Run the Load Events of gates `from..`.
    fn fire_loaded(&mut self, from: GateId) {
        for g in from..self.graph.gates.len() as GateId {
            if self.graph.gate(g).op == Op::EventLoaded {
                self.activate(g, Activation::Fire);
            }
        }
    }

    /// Run one tick.
    fn step(&mut self) -> Result<(), SimError> {
        self.tick += 1;
        self.dt = self.opts.dt_at(self.tick);
        self.uptime += self.dt;
        self.steps = 0;
        self.invocations = 0;
        self.unions_passed[0].clear();

        for (g, act) in std::mem::take(&mut self.carried) {
            self.push_late(g, act);
        }
        self.deliver_events();
        self.deliver_world_events();
        let tick = self.tick;
        let mut ended: Vec<(GateId, u16)> = self
            .pulses
            .iter()
            .filter(|&(_, &at)| at + 2 <= tick)
            .map(|(&k, _)| k)
            .collect();
        ended.sort_unstable();
        for (g, o) in ended {
            self.pulses.remove(&(g, o));
            self.set_out(g, o, Some(Value::Bool(false)));
        }
        // P23: a write fires the detector in the next tick's late phase.
        for w in std::mem::take(&mut self.container_fires) {
            self.push_late(w, Activation::Fire);
        }
        self.advance_buffers();
        self.advance_clocks()?;
        self.advance_timers()?;
        for g in self.time_sources.clone() {
            self.invalidate(g);
        }
        self.settle()?;
        self.late_pass()?;
        self.pass()
    }

    /// Run the work that lands ahead of the tick's handlers, in rank order,
    /// with anything it sets off.
    fn late_pass(&mut self) -> Result<(), SimError> {
        self.in_late = true;
        loop {
            let (g, act) = if let Some(mut entry) = self.late.first_entry() {
                let unit = entry.get_mut().pop_front().expect("late lists are never empty");
                if entry.get().is_empty() {
                    entry.remove();
                }
                unit
            } else if let Some(unit) = self.late_events.pop_front() {
                unit
            } else {
                break;
            };
            self.run_unit(g, act)?;
            self.finish_chain()?;
        }
        self.in_late = false;
        Ok(())
    }

    /// After a unit's chain: run its forks, publish its var writes, settle.
    fn finish_chain(&mut self) -> Result<(), SimError> {
        while let Some((fg, fi)) = self.forks.pop_front() {
            self.run_chain(fg, fi)?;
        }
        self.publish_writes();
        self.settle()
    }

    /// Run the pending units in rank order.
    fn pass(&mut self) -> Result<(), SimError> {
        self.cursor = None;
        while let Some(mut entry) = self.pending.first_entry() {
            let rank = *entry.key();
            let (g, act) = entry.get_mut().pop_front().expect("pending lists are never empty");
            if entry.get().is_empty() {
                entry.remove();
            }
            self.cursor = Some(rank);
            self.run_unit(g, act)?;
            self.finish_chain()?;
        }
        self.cursor = None;
        Ok(())
    }

    fn activate(&mut self, g: GateId, act: Activation) {
        let r = self.rank[g as usize];
        if self.in_late {
            self.push_late(g, act);
        } else if self.cursor.is_some_and(|c| r <= c) {
            self.carried.push((g, act));
        } else {
            self.push_pending(g, act);
        }
    }

    fn push_late(&mut self, g: GateId, act: Activation) {
        let r = self.rank[g as usize];
        self.late.entry(r).or_default().push_back((g, act));
    }

    fn push_pending(&mut self, g: GateId, act: Activation) {
        let r = self.rank[g as usize];
        self.pending.entry(r).or_default().push_back((g, act));
    }

    fn run_unit(&mut self, g: GateId, act: Activation) -> Result<(), SimError> {
        if self.graph.gate(g).op == Op::Dead {
            return Ok(());
        }
        match act {
            Activation::Pin(i) => self.run_chain(g, i),
            Activation::Fire => {
                let gate = self.graph.gate(g);
                let out = match gate.op {
                    Op::ChangeDetector => gate.output("OnChanged"),
                    Op::Clock => gate.output("Pulse"),
                    Op::ContainerWatch => gate.output("OnChanged"),
                    Op::Stopwatch => gate.output("Expired"),
                    Op::Tween => gate.output("Arrived"),
                    Op::BufferTicks | Op::BufferSeconds => gate.output("Output"),
                    _ => gate.output("ExecOut"),
                };
                if let Some(o) = out {
                    self.fire_chain(g, o)?;
                }
                Ok(())
            }
            Activation::Event(data) => {
                for (k, v) in data.into_iter().enumerate() {
                    if let Some(o) = self.graph.gate(g).output(&format!("DataOut{}", k + 1)) {
                        self.set_out(g, o, v);
                    }
                }
                if let Some(o) = self.graph.gate(g).output("ExecOut") {
                    self.fire_chain(g, o)?;
                }
                Ok(())
            }
            Activation::World(outputs, fire) => {
                for (name, v) in outputs {
                    self.set_out_named(g, &name, Some(v));
                }
                if let Some(o) = self.graph.gate(g).output(fire.as_deref().unwrap_or("ExecOut")) {
                    self.fire_chain(g, o)?;
                }
                Ok(())
            }
            Activation::Output(o) => self.fire_chain(g, o),
        }
    }

    /// Deliver the events the world raises this tick to their gates.
    fn deliver_world_events(&mut self) {
        for event in self.world.events(self.tick) {
            if let Some(g) = event.gate {
                self.push_late(g, Activation::World(event.outputs, event.fire));
                continue;
            }
            for g in 0..self.graph.gates.len() as GateId {
                let gate = self.graph.gate(g);
                if gate.op != Op::WorldEvent || gate.class != event.class {
                    continue;
                }
                if let Some((field, want)) = &event.filter
                    && gate.field_str(field).as_deref() != Some(want.as_str())
                {
                    continue;
                }
                self.push_late(g, Activation::World(event.outputs.clone(), event.fire.clone()));
            }
        }
    }

    /// Run gate `g` entered through input `i`, then its exec chain
    /// depth-first.
    fn run_chain(&mut self, g: GateId, i: u16) -> Result<(), SimError> {
        let mut stack: Vec<(GateId, u16)> = vec![(g, i)];
        while let Some((g, i)) = stack.pop() {
            self.steps += 1;
            self.total_steps += 1;
            if self.steps > MAX_STEPS_PER_TICK {
                return Err(self.error(g, "runaway exec chain: too many gates in one tick"));
            }
            if let Some(o) = self.exec_gate(g, i)? {
                self.push_calls(g, o, &mut stack);
            }
        }
        Ok(())
    }

    /// Fire exec output `o` of `g` and run what it calls.
    fn fire_chain(&mut self, g: GateId, o: u16) -> Result<(), SimError> {
        let mut stack = Vec::new();
        self.push_calls(g, o, &mut stack);
        while let Some((g, i)) = stack.pop() {
            self.steps += 1;
            self.total_steps += 1;
            if self.steps > MAX_STEPS_PER_TICK {
                return Err(self.error(g, "runaway exec chain: too many gates in one tick"));
            }
            if let Some(o) = self.exec_gate(g, i)? {
                self.push_calls(g, o, &mut stack);
            }
        }
        Ok(())
    }

    /// Queue the calls of exec output `o`: local ones onto `stack` so the
    /// first-stored runs first, each to completion; remote ones to run when
    /// the chain is done.
    fn push_calls(&mut self, g: GateId, o: u16, stack: &mut Vec<(GateId, u16)>) {
        if !self.planes.is_empty()
            && self.graph.gates[g as usize].outputs[o as usize]
                .calls
                .iter()
                .any(|c| self.frame_of(self.graph.gate(c.gate).addr.grid) != self.planes.len())
        {
            return self.push_calls_in_plane(g, o, stack);
        }
        let calls = &self.graph.gates[g as usize].outputs[o as usize].calls;
        for c in calls {
            if c.remote {
                self.forks.push_back((c.gate, c.input));
            }
        }
        for c in calls.iter().rev() {
            if !c.remote {
                stack.push((c.gate, c.input));
            }
        }
    }

    /// [`Self::push_calls`] inside a callable when some call leaves its plane.
    /// P24 to P26: an exec out of a loop body through a boundary pin runs
    /// nothing outside, but every Exec Union it reaches counts as having
    /// passed for that tick.
    fn push_calls_in_plane(&mut self, g: GateId, o: u16, stack: &mut Vec<(GateId, u16)>) {
        let calls = self.graph.gates[g as usize].outputs[o as usize].calls.clone();
        let depth = self.planes.len();
        for c in &calls {
            match self.frame_of(self.graph.gate(c.gate).addr.grid) {
                f if f == depth => {
                    if c.remote {
                        self.forks.push_back((c.gate, c.input));
                    }
                }
                f => self.lose(c.gate, f),
            }
        }
        for c in calls.iter().rev() {
            if !c.remote && self.frame_of(self.graph.gate(c.gate).addr.grid) == depth {
                stack.push((c.gate, c.input));
            }
        }
    }

    /// Which running frame `grid` belongs to: the innermost running plane
    /// that holds it (1 for the outermost), 0 for neither.
    fn frame_of(&self, grid: u32) -> usize {
        let mut at = grid;
        loop {
            if let Some(k) = self.planes.iter().rposition(|&p| p == at) {
                return k + 1;
            }
            match self.grid_parent.get(&at) {
                Some(&up) => at = up,
                None => return 0,
            }
        }
    }

    /// An exec lost leaving a plane reached `g`, which belongs to frame
    /// `frame`: mark the Unions it passes through as passed.
    fn lose(&mut self, g: GateId, frame: usize) {
        let mut work = vec![g];
        while let Some(g) = work.pop() {
            if self.graph.gate(g).op != Op::Union || !self.unions_passed[frame].insert(g) {
                continue;
            }
            if let Some(o) = self.graph.gate(g).output("ExecOut") {
                work.extend(self.graph.gate(g).outputs[o as usize].calls.iter().map(|c| c.gate));
            }
        }
    }

    /// Publish this chain's var writes to the gates off each var.
    fn publish_writes(&mut self) {
        for g in std::mem::take(&mut self.written) {
            self.written_set.remove(&g);
            let outside = self.written_outside.remove(&g);
            self.marking_late = self.written_in_plane.remove(&g) && !outside;
            if let Some(o) = self.graph.gate(g).output("Value") {
                self.mark_readers(g, o);
            }
        }
        self.marking_late = false;
    }

    /// Settle: re-read every watched input whose cone changed and act on it.
    fn settle(&mut self) -> Result<(), SimError> {
        for g in std::mem::take(&mut self.stale) {
            if self.dirty[g as usize] {
                self.refresh(g)?;
            }
        }
        while let Some((g, i)) = self.to_check.pop_front() {
            self.queued_check.remove(&(g, i));
            let late = self.late_checks.remove(&(g, i)) && !self.in_late;
            let v = self.read_input(g, i)?;
            let prev = self.last.insert((g, i), v.clone());
            let changed = prev.as_ref() != Some(&v);
            if !changed {
                continue;
            }
            let gate = self.graph.gate(g);
            match gate.op {
                Op::ChangeDetector if late => self.carried.push((g, Activation::Fire)),
                Op::ChangeDetector => self.activate(g, Activation::Fire),
                Op::Detector(d) => self.detect(g, d, prev.flatten().as_ref(), v.as_ref()),
                Op::Tween => self.tween_to(g, v),
                Op::BufferTicks | Op::BufferSeconds if gate.inputs[i as usize].name == "Input" => {
                    self.buffer_input(g, v);
                }
                _ => {
                    if v.as_ref().is_some_and(Value::truthy) {
                        if late {
                            self.carried.push((g, Activation::Pin(i)));
                        } else {
                            self.activate(g, Activation::Pin(i));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Whether input `i` of `g` is watched for changes: an Exec pin a value
    /// drives, a change detector's input, a buffer's value input.
    fn is_watched(&self, g: GateId, i: u16) -> bool {
        let gate = self.graph.gate(g);
        let input = &gate.inputs[i as usize];
        match gate.op {
            Op::ChangeDetector | Op::Detector(_) => input.name == "Input",
            Op::Tween => input.name == "Target",
            Op::BufferTicks | Op::BufferSeconds => input.name == "Input" && !self.buffer_exec_mode(g),
            _ => input.exec_pin && input.source.is_some(),
        }
    }

    fn buffer_exec_mode(&self, g: GateId) -> bool {
        matches!(&self.state[g as usize], State::Buffer(b) if b.exec_mode)
    }

    fn watch(&mut self, g: GateId, i: u16) {
        if self.queued_check.insert((g, i)) {
            self.to_check.push_back((g, i));
        }
    }

    /// Mark everything that reads output `o` of `g` stale, through any depth
    /// of pure gates, and queue the watched inputs it reaches.
    fn mark_readers(&mut self, g: GateId, o: u16) {
        let mut work: Vec<(GateId, u16)> = vec![(g, o)];
        while let Some((g, o)) = work.pop() {
            let readers = &self.graph.gates[g as usize].outputs[o as usize].readers;
            for &(r, ri) in readers {
                let reader = &self.graph.gates[r as usize];
                if reader.op.is_pure() {
                    if !self.dirty[r as usize] {
                        self.dirty[r as usize] = true;
                        self.stale.push(r);
                        for ro in 0..reader.outputs.len() as u16 {
                            work.push((r, ro));
                        }
                    }
                } else if self.is_watched(r, ri) {
                    if self.marking_late {
                        self.late_checks.insert((r, ri));
                    }
                    if self.queued_check.insert((r, ri)) {
                        self.to_check.push_back((r, ri));
                    }
                }
            }
        }
    }

    /// Mark pure gate `g` itself stale (a time source each tick).
    fn invalidate(&mut self, g: GateId) {
        if !self.dirty[g as usize] {
            self.dirty[g as usize] = true;
            self.stale.push(g);
        }
        for o in 0..self.graph.gate(g).outputs.len() as u16 {
            self.mark_readers(g, o);
        }
    }

    /// Set exec output value `o` of `g`, reaching its readers at once.
    fn set_out(&mut self, g: GateId, o: u16, v: Option<Value>) {
        let slot = &mut self.outs[g as usize][o as usize];
        if *slot != v {
            *slot = v;
            self.mark_readers(g, o);
        }
    }

    fn set_out_named(&mut self, g: GateId, name: &str, v: Option<Value>) {
        if let Some(o) = self.graph.gate(g).output(name) {
            self.set_out(g, o, v);
        }
    }

    // ---- reading values ----

    /// The value on input `i` of `g`: its wire's source, else what the save
    /// stores in the field.
    /// A poisoned value reaching a gate that is not pure raises the error it
    /// carries.
    fn read_input(&mut self, g: GateId, i: u16) -> Result<Option<Value>, SimError> {
        let input = &self.graph.gates[g as usize].inputs[i as usize];
        let v = match input.source {
            None => input.stored.clone(),
            Some(src) => self.read_output(src.gate, src.out)?,
        };
        if let Some(Value::Poison(p)) = &v
            && !self.graph.gate(g).op.is_pure()
        {
            return Err((*p.0).clone());
        }
        Ok(v)
    }

    fn read_named(&mut self, g: GateId, name: &str) -> Result<Option<Value>, SimError> {
        match self.graph.gate(g).input(name) {
            Some(i) => self.read_input(g, i),
            None => Ok(None),
        }
    }

    /// The current value of output `o` of `g`. A var's value is its live
    /// storage; a stale pure gate is recomputed first.
    fn read_output(&mut self, g: GateId, o: u16) -> Result<Option<Value>, SimError> {
        let gate = &self.graph.gates[g as usize];
        match gate.op {
            Op::Var => {
                return Ok(if gate.outputs[o as usize].name == "VarRef" {
                    Some(Value::Ref(g))
                } else {
                    match &self.state[g as usize] {
                        State::Var(v) => v.clone(),
                        _ => None,
                    }
                });
            }
            Op::ArrayVar | Op::MapVar | Op::FunctionDefinition => return Ok(Some(Value::Ref(g))),
            Op::Unsupported if !gate.inputs.iter().any(|i| i.exec_pin) => {
                let e = self.error(
                    g,
                    &format!("reads {}, a gate the simulator does not model", gate.outputs[o as usize].name),
                );
                return Ok(Some(Value::Poison(crate::value::Poisoned(std::sync::Arc::new(e)))));
            }
            _ => {}
        }
        if self.dirty[g as usize] {
            self.refresh(g)?;
        }
        let gate = self.graph.gate(g);
        if gate.op.is_pure()
            && !matches!(gate.op, Op::Reroute | Op::Assumed(_) | Op::WorldValue)
            && o != value_output(gate)
        {
            return Err(self.error(
                g,
                &format!("output {} is not certified", gate.outputs[o as usize].name),
            ));
        }
        Ok(self.outs[g as usize][o as usize].clone())
    }

    /// Recompute stale pure gate `g` and every stale pure gate it reads,
    /// sources first.
    fn refresh(&mut self, root: GateId) -> Result<(), SimError> {
        let mut stack: Vec<(GateId, bool)> = vec![(root, false)];
        while let Some((g, expanded)) = stack.pop() {
            if !self.dirty[g as usize] {
                continue;
            }
            if expanded {
                self.recompute(g);
                self.total_evals += 1;
                self.dirty[g as usize] = false;
                continue;
            }
            stack.push((g, true));
            for input in &self.graph.gates[g as usize].inputs {
                if let Some(src) = input.source
                    && self.dirty[src.gate as usize]
                    && self.graph.gate(src.gate).op.is_pure()
                {
                    if stack.len() > 10_000_000 {
                        return Err(self.error(g, "pure gates form a cycle"));
                    }
                    stack.push((src.gate, false));
                }
            }
        }
        Ok(())
    }

    /// Recompute pure gate `g`. A value it cannot compute becomes a poisoned
    /// value, raised when something not pure reads it.
    fn recompute(&mut self, g: GateId) {
        let value = match self.compute(g) {
            Ok(v) => v,
            Err(e) => {
                let poison = Some(Value::Poison(crate::value::Poisoned(std::sync::Arc::new(e))));
                if let Op::Assumed(_) | Op::WorldValue = self.graph.gate(g).op {
                    for slot in &mut self.outs[g as usize] {
                        slot.clone_from(&poison);
                    }
                }
                poison
            }
        };
        let main = value_output(self.graph.gate(g));
        if let Some(slot) = self.outs[g as usize].get_mut(main as usize) {
            *slot = value;
        }
    }

    fn compute(&mut self, g: GateId) -> Result<Option<Value>, SimError> {
        let gate = &self.graph.gates[g as usize];
        let value = match gate.op {
            Op::Reroute => self.read_named(g, "RER_Input")?,
            Op::WorldValue => {
                let tick = self.tick;
                let mut world = std::mem::replace(
                    &mut self.world,
                    Box::new(crate::world::NullWorld::default()),
                );
                let outs = world.component_outputs(tick, g, &mut Inputs { m: self, g });
                self.world = world;
                let main = value_output(self.graph.gate(g));
                let mut value = None;
                for (name, v) in outs.map_err(|e| self.error(g, &e))? {
                    match self.graph.gate(g).output(&name) {
                        Some(o) if o == main => value = Some(v),
                        Some(o) => self.outs[g as usize][o as usize] = Some(v),
                        None => {}
                    }
                }
                value
            }
            Op::Assumed(law) => {
                let mut inputs = Vec::with_capacity(law.inputs().len());
                for name in law.inputs() {
                    let v = self.read_named(g, name)?;
                    if let Some(Value::Poison(_)) = v {
                        return Ok(v);
                    }
                    inputs.push(v);
                }
                let gate = self.graph.gate(g);
                let outs = crate::assumed::eval(law, gate, &inputs).map_err(|e| self.error(g, &e))?;
                let main = value_output(gate);
                let mut value = None;
                for (name, mut v) in outs {
                    if let Some(Value::Str(s)) = &mut v {
                        cap_string(s);
                    }
                    match self.graph.gate(g).output(name) {
                        Some(o) if o == main => value = v,
                        Some(o) => self.outs[g as usize][o as usize] = v,
                        None => {}
                    }
                }
                value
            }
            Op::DeltaTime => Some(Value::Float(self.dt)),
            Op::ServerUptime => Some(Value::Float(self.uptime)),
            Op::UnixEpoch => Some(Value::Int(START_EPOCH + self.uptime.floor() as i64)),
            // The save holds an index into its asset table: one object per
            // asset, so references to the same asset compare equal.
            Op::AssetRef => Some(match gate.field("Asset") {
                Some(brdb::schema::BrdbValue::Asset(Some(i))) => Value::Object(ASSET_OBJECTS + *i as u64),
                _ => Value::Object(0),
            }),
            Op::Select => {
                let cond = self.read_named(g, "bSelectB")?;
                if cond.as_ref().is_some_and(Value::truthy) {
                    self.read_named(g, "InputB")?
                } else {
                    self.read_named(g, "InputA")?
                }
            }
            Op::FormatText => {
                let template = gate.field_str("FormatString").unwrap_or_default();
                let mut inputs = Vec::with_capacity(7);
                for slot in ["InputA", "InputB", "InputC", "InputD", "InputE", "InputF", "InputG"] {
                    let v = self.read_named(g, slot)?;
                    if let Some(Value::Poison(_)) = v {
                        return Ok(v);
                    }
                    match v.as_ref().map(Value::to_semantic) {
                        None => inputs.push(None),
                        Some(Some(s)) => inputs.push(Some(s)),
                        Some(None) => {
                            return Err(self.error(
                                g,
                                &format!("FormatText of a {} is not certified", v.map_or("", |v| v.kind())),
                            ));
                        }
                    }
                }
                match semantics::format_text_sim(&template, &inputs) {
                    Some(s) => Some(Value::Str(s)),
                    None => {
                        return Err(self.error(
                            g,
                            &format!("FormatText {template:?} is not certified for these inputs"),
                        ));
                    }
                }
            }
            Op::Concat => {
                if let Some(sep) = self.read_named(g, "Separator")?
                    && sep != Value::Str(String::new())
                {
                    return Err(self.error(g, "Concatenate with a separator is not certified"));
                }
                let a = self.read_named(g, "InputA")?;
                let b = self.read_named(g, "InputB")?;
                self.eval_certified(g, &[a, b])?
            }
            Op::EnumToInteger => self.read_named(g, "Input")?.map(|v| match v.as_int() {
                Some(n) => Value::Int(n),
                None => v,
            }),
            Op::IntegerToEnum => match self.read_named(g, "Input")? {
                Some(Value::Poison(p)) => Some(Value::Poison(p)),
                v => v.and_then(|v| v.as_int()).map(Value::Int),
            },
            Op::Eval if handle_compare(gate) => {
                let a = self.read_named(g, "InputA")?;
                let b = self.read_named(g, "InputB")?;
                if is_handle(a.as_ref()) || is_handle(b.as_ref()) {
                    let equal = self.graph.gate(g).class.ends_with("CompareEqual");
                    Some(Value::Bool((a == b) == equal))
                } else {
                    self.eval_certified(g, &[a, b])?
                }
            }
            Op::Eval => {
                let ports = std::mem::take(&mut self.eval_inputs[g as usize]);
                let mut inputs = Vec::with_capacity(ports.len());
                let mut read = Ok(());
                for &p in &ports {
                    match p {
                        Some(i) => match self.read_input(g, i) {
                            Ok(v) => inputs.push(v),
                            Err(e) => {
                                read = Err(e);
                                break;
                            }
                        },
                        None => inputs.push(None),
                    }
                }
                self.eval_inputs[g as usize] = ports;
                read?;
                self.eval_certified(g, &inputs)?
            }
            _ => return Err(self.error(g, "not a pure gate")),
        };
        let mut value = value;
        if let Some(Value::Str(s)) = &mut value {
            cap_string(s);
        }
        Ok(value)
    }

    fn eval_certified(
        &self,
        g: GateId,
        inputs: &[Option<Value>],
    ) -> Result<Option<Value>, SimError> {
        if let Some(p) = inputs.iter().flatten().find(|v| matches!(v, Value::Poison(_))) {
            return Ok(Some(p.clone()));
        }
        let mut sem = Vec::with_capacity(inputs.len());
        for v in inputs {
            match v {
                None => sem.push(None),
                Some(v) => match v.to_semantic() {
                    Some(s) => sem.push(Some(s)),
                    None => {
                        return Err(self.error(
                            g,
                            &format!("not certified for a {} input", v.kind()),
                        ));
                    }
                },
            }
        }
        let class = &self.graph.gate(g).class;
        match semantics::eval_sim(class, &sem) {
            Some(v) => Ok(Some(Value::from_semantic(v))),
            None => {
                let kinds: Vec<&str> = inputs
                    .iter()
                    .map(|v| v.as_ref().map_or("unwired", Value::kind))
                    .collect();
                Err(self.error(
                    g,
                    &format!("not certified for inputs ({})", kinds.join(", ")),
                ))
            }
        }
    }

    // ---- exec gates ----

    /// The storage gate a reference input names.
    fn target(&mut self, g: GateId, port: &str) -> Result<GateId, SimError> {
        match self.read_named(g, port)? {
            Some(Value::Ref(t)) => Ok(t),
            other => Err(self.error(
                g,
                &format!(
                    "{port} holds {}, not a reference",
                    other.as_ref().map_or("nothing".to_string(), |v| v.kind().to_string())
                ),
            )),
        }
    }

    fn record_write(&mut self, var: GateId) {
        if self.depth > 0 {
            self.written_in_plane.insert(var);
        } else {
            self.written_outside.insert(var);
        }
        if self.written_set.insert(var) {
            self.written.push(var);
        }
    }

    /// Run exec gate `g` entered through input `i`. Returns the exec output
    /// to fire next.
    fn exec_gate(&mut self, g: GateId, i: u16) -> Result<Option<u16>, SimError> {
        let gate = &self.graph.gates[g as usize];
        let op = gate.op;
        let exec_out = gate.output("ExecOut");
        match op {
            Op::Dead => return Ok(None),
            Op::PrefabSpawner => {
                if self.graph.gate(g).inputs[i as usize].name == "DestroyAll" {
                    if let State::Spawner(copies) = &mut self.state[g as usize] {
                        for c in std::mem::take(copies) {
                            self.destroy(c.gates);
                        }
                    }
                    return Ok(None);
                }
                let limit = self.read_named(g, "Limit")?.and_then(|v| v.as_int()).unwrap_or(0);
                let lifetime = self.read_named(g, "Lifetime")?.and_then(|v| v.as_float()).unwrap_or(0.0);
                if limit > 0
                    && let State::Spawner(copies) = &mut self.state[g as usize]
                    && copies.len() as i64 >= limit
                {
                    let oldest = copies.remove(0);
                    self.destroy(oldest.gates);
                }
                let entity = self.spawn(g)?;
                let ends_at = (lifetime > 0.0).then_some(self.uptime + lifetime);
                if let State::Spawner(copies) = &mut self.state[g as usize] {
                    copies.push(SpawnedCopy { ends_at, ..entity.clone() });
                }
                self.set_out_named(g, "Entity", Some(Value::Object(entity.entity)));
            }
            Op::DestroyPrefab => {
                let copy = match self.read_named(g, "Entity")? {
                    Some(Value::Object(id)) => self.destroy_entity(id),
                    _ => false,
                };
                // Anything but a spawned copy is the world's to destroy.
                if !copy {
                    return self.world_gate(g);
                }
            }
            Op::Reroute => return Ok(gate.output("RER_Output")),
            Op::Union => {
                if !self.unions_passed.last_mut().expect("the tick frame").insert(g) {
                    return Ok(None);
                }
            }
            Op::VarGet => {
                let var = self.target(g, "VarRef")?;
                let v = match &self.state[var as usize] {
                    State::Var(v) => v.clone(),
                    _ => return Err(self.error(g, "VarRef does not name a Variable")),
                };
                self.set_out_named(g, "Value", v);
            }
            Op::VarSet => {
                let var = self.target(g, "VarRef")?;
                let mut v = self.read_named(g, "Value")?;
                if self.graph.gate(g).field_bool("bChangeType") != Some(true)
                    && let State::Var(cur) = &self.state[var as usize]
                {
                    v = set_in_kind(cur.as_ref(), v).map_err(|what| self.error(g, &what))?;
                }
                match &mut self.state[var as usize] {
                    State::Var(slot) => *slot = v,
                    _ => return Err(self.error(g, "VarRef does not name a Variable")),
                }
                self.record_write(var);
            }
            Op::VarIncrement => {
                let var = self.target(g, "VarRef")?;
                let by = self.read_named(g, "Value")?;
                let cur = match &self.state[var as usize] {
                    State::Var(v) => v.clone(),
                    _ => return Err(self.error(g, "VarRef does not name a Variable")),
                };
                let sum = self.eval_class(g, "BrickComponentType_Expr_MathAdd", &[cur, by])?;
                if let State::Var(slot) = &mut self.state[var as usize] {
                    *slot = sum;
                }
                self.record_write(var);
            }
            Op::Branch => {
                let cond = self.read_named(g, "bCond")?;
                let gate = self.graph.gate(g);
                return Ok(if cond.as_ref().is_some_and(Value::truthy) {
                    gate.output("ExecOutA")
                } else {
                    gate.output("ExecOutB")
                });
            }
            Op::BroadcastChat => {
                let msg = self.read_named(g, "Message")?;
                let text = match msg {
                    None => String::new(),
                    Some(Value::Str(s)) => s,
                    Some(v) if v.to_semantic().is_some() => v.render(),
                    Some(v) => {
                        return Err(self.error(g, &format!("printing a {} is not certified", v.kind())));
                    }
                };
                self.world.chat(self.tick, &text);
            }
            Op::SendEvent { global } => {
                let gate = self.graph.gate(g);
                let name = gate.field_str("EventName").unwrap_or_default();
                if self.read_named(g, "Target")?.is_some_and(|t| t != Value::Object(0)) {
                    return Err(self.error(g, "a targeted custom event needs a scripted world"));
                }
                let mut data = Vec::with_capacity(8);
                for k in 1..=8 {
                    data.push(self.read_named(g, &format!("DataIn{k}"))?);
                }
                if self.outbox.iter().any(|(s, _)| *s == g) {
                    // One send gate fired again on the same tick: the game caps it.
                } else {
                    self.outbox.push((g, Send { name, global, data }));
                }
            }
            Op::BufferTicks | Op::BufferSeconds => {
                let countdown = self.countdown(g, true);
                if let State::Buffer(b) = &mut self.state[g as usize] {
                    b.fires.push_back(countdown);
                }
                return Ok(None);
            }
            Op::Random => {
                let (lo, hi) = (self.read_named(g, "Min")?, self.read_named(g, "Max")?);
                let v = self.random_between(lo, hi).map_err(|e| self.error(g, &e))?;
                self.set_out_named(g, "Output", Some(v));
            }
            Op::Toggle => {
                if let Some(o) = self.graph.gate(g).output("Output") {
                    let on = self.outs[g as usize][o as usize].as_ref().is_some_and(Value::truthy);
                    self.set_out(g, o, Some(Value::Bool(!on)));
                }
            }
            Op::Cycle => {
                let down = self.graph.gate(g).inputs[i as usize].name == "ExecDown";
                let count = self.read_named(g, "Count")?.and_then(|v| v.as_int()).unwrap_or(0);
                if let Some(o) = self.graph.gate(g).output("Output") {
                    let now = self.outs[g as usize][o as usize].as_ref().and_then(Value::as_int).unwrap_or(0);
                    let step = if down { -1 } else { 1 };
                    let next = if count <= 0 { 0 } else { (now + step).rem_euclid(count) };
                    self.set_out(g, o, Some(Value::Int(next)));
                }
                if down {
                    return Ok(self.graph.gate(g).output("ExecOutDown"));
                }
            }
            Op::PrintToConsole => {
                let text = match self.read_named(g, "Text")? {
                    None => String::new(),
                    Some(Value::Str(s)) => s,
                    Some(v) => v.render(),
                };
                self.world.chat(self.tick, &format!("[console] {text}"));
            }
            Op::Stopwatch => {
                let pin = self.graph.gate(g).inputs[i as usize].name.clone();
                if let State::Stopwatch { time, running } = &mut self.state[g as usize] {
                    match pin.as_str() {
                        "Restart" => {
                            *time = 0.0;
                            *running = true;
                        }
                        "Pause" => *running = false,
                        "Resume" => *running = true,
                        _ => {}
                    }
                }
                if pin == "Restart" {
                    self.set_out_named(g, "Time", Some(Value::Float(0.0)));
                }
                return Ok(None);
            }
            Op::Queue { seconds } => {
                let mut record = Vec::with_capacity(8);
                for k in 1..=8 {
                    record.push(self.read_named(g, &format!("DataIn{k}"))?);
                }
                let countdown = if seconds {
                    let wait = self.read_named(g, "SecondsToWait")?.and_then(|v| v.as_float());
                    Countdown::Seconds {
                        wait: wait.unwrap_or(0.0) as f32,
                        elapsed: 0.0,
                    }
                } else {
                    let ticks = self.read_named(g, "TicksToWait")?.and_then(|v| v.as_int());
                    Countdown::Ticks(ticks.unwrap_or(0).max(1))
                };
                if let State::Queue(q) = &mut self.state[g as usize]
                    && q.len() < MAX_QUEUE_RECORDS
                {
                    q.push_back((record, countdown));
                }
                return Ok(None);
            }
            Op::Array(a) => self.array_op(g, a)?,
            Op::FunctionCall => return self.function_call(g),
            Op::Loop(kind) => return self.run_loop(g, kind),
            Op::CallableBreak => self.break_requested = true,
            Op::Map(m) => self.map_op(g, m)?,
            _ => return self.world_gate(g),
        }
        let _ = i;
        Ok(exec_out)
    }

    fn eval_class(
        &self,
        g: GateId,
        class: &str,
        inputs: &[Option<Value>],
    ) -> Result<Option<Value>, SimError> {
        let sem: Vec<_> = inputs
            .iter()
            .map(|v| v.as_ref().and_then(Value::to_semantic))
            .collect();
        semantics::eval_sim(class, &sem)
            .map(|v| Some(Value::from_semantic(v)))
            .ok_or_else(|| self.error(g, &format!("{class} is not certified for these inputs")))
    }

    // ---- timers and events ----

    /// The countdown a buffer starts for value `v`.
    fn countdown(&self, g: GateId, rising: bool) -> Countdown {
        let gate = self.graph.gate(g);
        if gate.op == Op::BufferSeconds {
            let wait = gate.field_f64("SecondsToWait").unwrap_or(0.0);
            let zero = gate.field_f64("ZeroSecondsToWait").unwrap_or(-1.0);
            let w = if !rising && zero >= 0.0 { zero } else { wait };
            Countdown::Seconds {
                wait: w as f32,
                elapsed: 0.0,
            }
        } else {
            let ticks = gate.field_i64("TicksToWait").unwrap_or(0);
            let zero = gate.field_i64("ZeroTicksToWait").unwrap_or(-1);
            let t = if !rising && zero >= 0 { zero } else { ticks };
            Countdown::Ticks(t.max(1))
        }
    }

    /// Load a copy of spawner `g`'s prefab after every gate, set it up as
    /// load sets up a save, and run its Load Event this tick.
    fn spawn(&mut self, g: GateId) -> Result<SpawnedCopy, SimError> {
        let path = self.graph.gate(g).field_str("Prefab").unwrap_or_default();
        let Some(bytes) = self.graph.prefabs.get(&path).cloned() else {
            return Err(self.error(g, &format!("the save holds no prefab at {path:?}")));
        };
        let copy = crate::load::read_brz(&bytes)
            .and_then(Graph::build)
            .map_err(|e| self.error(g, &format!("the prefab does not load: {e}")))?;
        if self.graph.gates.len() + copy.gates.len() > MAX_GATES {
            return Err(self.error(g, "spawning grew the run past twenty million gates"));
        }
        let from = self.graph.gates.len() as GateId;
        let offset = self.graph.append(copy);
        self.copy_offsets.push((from, offset));
        self.add_gates(from);
        self.resolve_handles(from, offset);
        self.settle_new(from)?;
        self.fire_loaded(from);
        self.spawned += 1;
        Ok(SpawnedCopy {
            entity: SPAWN_OBJECTS + self.spawned,
            gates: from..self.graph.gates.len() as GateId,
            ends_at: None,
        })
    }

    /// Run world gate `g`: the world computes its outputs and the exec
    /// output to fire.
    fn world_gate(&mut self, g: GateId) -> Result<Option<u16>, SimError> {
        let tick = self.tick;
        let mut world = std::mem::replace(&mut self.world, Box::new(crate::world::NullWorld::default()));
        let result = world.exec_gate(tick, g, &mut Inputs { m: self, g });
        self.world = world;
        let result = result.map_err(|e| self.error(g, &e))?;
        for (name, v) in result.outputs {
            self.set_out_named(g, &name, Some(v));
        }
        for (port, items) in result.arrays {
            self.fill_array(g, &port, items)?;
        }
        Ok(result.fire.and_then(|f| self.graph.gate(g).output(&f)))
    }

    /// Destroy the spawned copy whose entity is `id`, wherever it came from;
    /// false when no spawner holds it.
    fn destroy_entity(&mut self, id: u64) -> bool {
        for s in 0..self.state.len() {
            let State::Spawner(copies) = &mut self.state[s] else { continue };
            if let Some(k) = copies.iter().position(|c| c.entity == id) {
                let c = copies.remove(k);
                self.destroy(c.gates);
                return true;
            }
        }
        false
    }

    /// Make the gates of a copy inert: they keep their ids but do nothing,
    /// and nothing waiting for them runs. A copy a destroyed copy spawned
    /// goes with it.
    fn destroy(&mut self, gates: std::ops::Range<GateId>) {
        let mut inner = Vec::new();
        for g in gates.clone() {
            if let State::Spawner(copies) = std::mem::replace(&mut self.state[g as usize], State::None) {
                inner.extend(copies.into_iter().map(|c| c.gates));
            }
            self.graph.gates[g as usize].op = Op::Dead;
            self.dirty[g as usize] = false;
        }
        for list in self.receivers.values_mut() {
            list.retain(|g| !gates.contains(g));
        }
        self.timed.retain(|g| !gates.contains(g));
        self.time_sources.retain(|g| !gates.contains(g));
        self.container_watchers.retain(|g| !gates.contains(g));
        for c in inner {
            self.destroy(c);
        }
    }

    /// Container `c` was written: its detectors fire next tick.
    fn container_changed(&mut self, c: GateId) -> Result<(), SimError> {
        for w in self.container_watchers.clone() {
            if self.read_named(w, "ContainerRef")? == Some(Value::Ref(c)) && !self.container_fires.contains(&w) {
                self.container_fires.push(w);
            }
        }
        Ok(())
    }

    /// A value between `lo` and `hi` of their kind: an integer in the closed
    /// range, a number in the half-open one, a vector per component.
    fn random_between(&mut self, lo: Option<Value>, hi: Option<Value>) -> Result<Value, String> {
        Ok(match (lo, hi) {
            (Some(Value::Int(a)), Some(Value::Int(b))) => {
                let (a, b) = (a.min(b), a.max(b));
                let span = (i128::from(b) - i128::from(a) + 1) as u128;
                let pick = (u128::from(self.rng.next()) % span) as i128;
                Value::Int((i128::from(a) + pick) as i64)
            }
            (Some(Value::Vector { x, y, z }), Some(Value::Vector { x: x2, y: y2, z: z2 })) => Value::Vector {
                x: x + self.rng.unit() * (x2 - x),
                y: y + self.rng.unit() * (y2 - y),
                z: z + self.rng.unit() * (z2 - z),
            },
            (a, b) => {
                let num = |v: &Option<Value>, unwired: f64| match v {
                    None => Some(unwired),
                    Some(v) => v.as_float(),
                };
                match (num(&a, 0.0), num(&b, 1.0)) {
                    (Some(a), Some(b)) => Value::Float(a + self.rng.unit() * (b - a)),
                    _ => return Err("Random between these kinds of value is not modeled".into()),
                }
            }
        })
    }

    /// Tween `g`'s target became `v`: run from where it is now.
    fn tween_to(&mut self, g: GateId, v: Option<Value>) {
        let now = self.graph.gate(g).output("Value").and_then(|o| self.outs[g as usize][o as usize].clone());
        if let State::Tween(t) = &mut self.state[g as usize] {
            t.start = now;
            t.target = v;
            t.elapsed = 0.0;
            t.running = true;
        }
    }

    /// Stopwatches, tweens and dampers take this tick's `dt`.
    fn advance_timers(&mut self) -> Result<(), SimError> {
        let dt = self.dt;
        for g in self.timed.clone() {
            match self.graph.gate(g).op {
                Op::Stopwatch => {
                    let limit = self.read_named(g, "Limit")?.and_then(|v| v.as_float()).unwrap_or(0.0);
                    let State::Stopwatch { time, running } = &mut self.state[g as usize] else { continue };
                    if !*running {
                        continue;
                    }
                    *time += dt;
                    let expired = limit > 0.0 && *time >= limit;
                    if expired {
                        *time = limit;
                        *running = false;
                    }
                    let shown = *time;
                    self.set_out_named(g, "Time", Some(Value::Float(shown)));
                    if expired {
                        self.push_late(g, Activation::Fire);
                    }
                }
                Op::Tween => {
                    let duration = self.read_named(g, "Duration")?.and_then(|v| v.as_float()).unwrap_or(0.0);
                    let function = self.read_named(g, "Function")?.and_then(|v| v.as_int()).unwrap_or(0);
                    let direction = self.read_named(g, "Direction")?.and_then(|v| v.as_int()).unwrap_or(0);
                    let State::Tween(t) = &mut self.state[g as usize] else { continue };
                    if !t.running {
                        continue;
                    }
                    t.elapsed += dt;
                    let f = if duration > 0.0 { (t.elapsed / duration).min(1.0) } else { 1.0 };
                    let arrived = f >= 1.0;
                    if arrived {
                        t.running = false;
                    }
                    let (start, target) = (t.start.clone(), t.target.clone());
                    let value = if arrived {
                        target
                    } else {
                        let eased = crate::assumed::ease((function, direction), f);
                        Some(crate::assumed::lerp(start, target, eased).map_err(|e| self.error(g, &e))?)
                    };
                    self.set_out_named(g, "Value", value);
                    if arrived {
                        self.push_late(g, Activation::Fire);
                    }
                }
                Op::PrefabSpawner => {
                    let now = self.uptime;
                    let State::Spawner(copies) = &mut self.state[g as usize] else { continue };
                    let (ended, live): (Vec<SpawnedCopy>, Vec<SpawnedCopy>) =
                        std::mem::take(copies).into_iter().partition(|c| c.ends_at.is_some_and(|t| t <= now));
                    *copies = live;
                    for c in ended {
                        self.destroy(c.gates);
                    }
                }
                Op::Dampen => {
                    let target = self.read_named(g, "Target")?;
                    let smooth = self.read_named(g, "SmoothTime")?.and_then(|v| v.as_float()).unwrap_or(0.0);
                    let now = self.graph.gate(g).output("Value").and_then(|o| self.outs[g as usize][o as usize].clone());
                    let State::Dampen(velocity) = &mut self.state[g as usize] else { continue };
                    let next = dampen(now.as_ref(), target, smooth, dt, velocity);
                    self.set_out_named(g, "Value", next);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Detector `g` saw its input change from `prev` to `v`. The first value
    /// it sees seeds it without a pulse (the save's `bSeeded`).
    fn detect(&mut self, g: GateId, d: Detector, prev: Option<&Value>, v: Option<&Value>) {
        let number = |v: Option<&Value>| v.and_then(Value::as_float).unwrap_or(0.0);
        let (was, now) = (number(prev), number(v));
        let side = match d {
            Detector::Change => "bPulseOnChange",
            Detector::Edge if now > was => "bPulseOnRisingEdge",
            Detector::Edge if now < was => "bPulseOnFallingEdge",
            Detector::EdgeExec if now > was => "OnRisingEdge",
            Detector::EdgeExec if now < was => "OnFallingEdge",
            _ => return,
        };
        let Some(o) = self.graph.gate(g).output(side) else {
            return;
        };
        if d == Detector::EdgeExec {
            self.activate(g, Activation::Output(o));
        } else {
            self.set_out(g, o, Some(Value::Bool(true)));
            self.pulses.insert((g, o), self.tick);
        }
    }

    /// A buffer's value input changed to `v`.
    fn buffer_input(&mut self, g: GateId, v: Option<Value>) {
        let rising = v.as_ref().is_some_and(Value::truthy);
        let countdown = self.countdown(g, rising);
        if let State::Buffer(b) = &mut self.state[g as usize] {
            if b.in_flight.is_some() {
                b.has_queued = true;
                b.queued = v;
            } else {
                b.in_flight = Some((v, countdown));
            }
        }
    }

    /// Count every running buffer down by one tick and land what is due.
    fn advance_buffers(&mut self) {
        let dt = self.dt as f32;
        for g in self.timed.clone() {
            if let State::Queue(q) = &mut self.state[g as usize] {
                for (_, c) in q.iter_mut() {
                    tick_down(c, dt);
                }
                // One record a tick, oldest first.
                if q.front().is_some_and(|(_, c)| due(c)) {
                    let (record, _) = q.pop_front().expect("checked above");
                    for (k, v) in record.into_iter().enumerate() {
                        self.set_out_named(g, &format!("DataOut{}", k + 1), v);
                    }
                    self.push_late(g, Activation::Fire);
                }
                continue;
            }
            let State::Buffer(b) = &mut self.state[g as usize] else {
                continue;
            };
            let mut fire = 0;
            for c in &mut b.fires {
                tick_down(c, dt);
            }
            while b.fires.front().is_some_and(due) {
                b.fires.pop_front();
                fire += 1;
            }
            let mut landed = None;
            if let Some((_, c)) = &mut b.in_flight {
                tick_down(c, dt);
                if due(c) {
                    landed = b.in_flight.take().map(|(v, _)| v);
                }
            }
            if let Some(v) = landed {
                let queued = std::mem::take(&mut b.has_queued).then(|| b.queued.take());
                if let Some(o) = self.graph.gate(g).output("Output") {
                    self.set_out(g, o, v);
                }
                if let Some(q) = queued {
                    let rising = q.as_ref().is_some_and(Value::truthy);
                    let c = self.countdown(g, rising);
                    if let State::Buffer(b) = &mut self.state[g as usize] {
                        b.in_flight = Some((q, c));
                    }
                }
            }
            for _ in 0..fire {
                self.push_late(g, Activation::Fire);
            }
        }
    }

    /// Clocks accumulate this tick's `dt` and fire on reaching the interval.
    fn advance_clocks(&mut self) -> Result<(), SimError> {
        let dt = self.dt;
        for g in self.timed.clone() {
            if self.graph.gate(g).op != Op::Clock {
                continue;
            }
            let enabled = self.read_named(g, "bEnabled")?.is_none_or(|v| v.truthy());
            let interval = match self.read_named(g, "IntervalSeconds")? {
                Some(v) => v.as_float().unwrap_or(1.0),
                None => self.graph.gate(g).field_f64("IntervalSeconds").unwrap_or(1.0),
            };
            let State::Clock { elapsed } = &mut self.state[g as usize] else {
                continue;
            };
            if !enabled {
                continue;
            }
            *elapsed += dt;
            // It starts again from zero, dropping the overshoot: 102 fires
            // over 1.28 s of 4 ms ticks at 0.01 s, not 127 (P19b).
            if *elapsed >= interval {
                *elapsed = 0.0;
                self.activate(g, Activation::Fire);
            }
        }
        Ok(())
    }

    /// Deliver last tick's custom events to their receivers.
    fn deliver_events(&mut self) {
        let mut sends = std::mem::take(&mut self.outbox);
        if self.opts.seed.is_some() {
            self.rng.shuffle(&mut sends);
        }
        for (_, send) in sends {
            let Some(receivers) = self.receivers.get(&send.name) else {
                continue;
            };
            for &r in &receivers.clone() {
                let global = matches!(self.graph.gate(r).op, Op::EventCustom { global: true });
                if global == send.global {
                    self.late_events.push_back((r, Activation::Event(send.data.clone())));
                }
            }
        }
    }

    pub(crate) fn error(&self, g: GateId, msg: &str) -> SimError {
        SimError {
            tick: self.tick,
            gate: Some(g),
            at: self.graph.describe(g),
            span: self.graph.gate(g).span.clone(),
            message: msg.to_string(),
        }
    }
}

/// The saved last input a seeded `EdgeExec` compares its first input with, so
/// a value that differs fires at load (P20g: `up@0` for a var starting at 10).
/// Edge and Changed take their first value without a pulse (P21: none for a
/// float from 10, a string from "x" or a bool from true).
fn seeded_last(gate: &Gate) -> Option<Value> {
    if gate.op != Op::Detector(Detector::EdgeExec) || gate.field_bool("bSeeded") != Some(true) {
        return None;
    }
    Some(Value::Float(gate.field_f64("InputLast").unwrap_or(0.0)))
}

/// One step of a critically damped follow from `now` toward `target`
/// (the usual smooth-damp), per component, keeping each component's velocity.
fn dampen(now: Option<&Value>, target: Option<Value>, smooth: f64, dt: f64, velocity: &mut [f64; 4]) -> Option<Value> {
    let parts = |v: Option<&Value>| -> Option<Vec<f64>> {
        Some(match v? {
            Value::Vector { x, y, z } => vec![*x, *y, *z],
            Value::Rotator { pitch, yaw, roll } => vec![*pitch, *yaw, *roll],
            Value::Color { r, g, b, a } => vec![*r, *g, *b, *a],
            Value::Quat { x, y, z, w } => vec![*x, *y, *z, *w],
            v => vec![v.as_float()?],
        })
    };
    let (Some(from), Some(to)) = (parts(now).or_else(|| parts(target.as_ref()).map(|t| vec![0.0; t.len()])), parts(target.as_ref())) else {
        return target;
    };
    if smooth <= 0.0 || from.len() != to.len() {
        *velocity = [0.0; 4];
        return target;
    }
    let omega = 2.0 / smooth;
    let x = omega * dt;
    let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let mut out = Vec::with_capacity(to.len());
    for (k, (&c, &t)) in from.iter().zip(&to).enumerate() {
        let change = c - t;
        let temp = (velocity[k] + omega * change) * dt;
        velocity[k] = (velocity[k] - omega * temp) * decay;
        let mut next = t + (change + temp) * decay;
        if (t - c > 0.0) == (next > t) {
            next = t;
            velocity[k] = 0.0;
        }
        out.push(next);
    }
    Some(match target? {
        Value::Vector { .. } => Value::Vector { x: out[0], y: out[1], z: out[2] },
        Value::Rotator { .. } => Value::Rotator { pitch: out[0], yaw: out[1], roll: out[2] },
        Value::Color { .. } => Value::Color { r: out[0], g: out[1], b: out[2], a: out[3] },
        Value::Quat { .. } => Value::Quat { x: out[0], y: out[1], z: out[2], w: out[3] },
        Value::Int(_) => Value::Int(out[0].round() as i64),
        _ => Value::Float(out[0]),
    })
}

/// The output a pure gate's law gives: `Output`, `Value` or `bOutput`,
/// else its first.
fn value_output(gate: &Gate) -> u16 {
    ["Output", "Value", "bOutput"]
        .iter()
        .find_map(|n| gate.output(n))
        .unwrap_or(0)
}

/// A compare gate that can see two handles: equal when they name the same
/// thing. Assumed from the fn suite, which compares bricks and passes in game.
fn handle_compare(gate: &Gate) -> bool {
    gate.class.ends_with("CompareEqual") || gate.class.ends_with("CompareNotEqual")
}

fn is_handle(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Ref(_) | Value::Object(_) | Value::Brick(_) | Value::Handle(_)))
}

fn tick_down(c: &mut Countdown, dt: f32) {
    match c {
        Countdown::Ticks(n) => *n -= 1,
        Countdown::Seconds { elapsed, .. } => *elapsed += dt,
    }
}

fn due(c: &Countdown) -> bool {
    match *c {
        Countdown::Ticks(n) => n <= 0,
        Countdown::Seconds { wait, elapsed } => elapsed >= wait,
    }
}

fn is_unit(graph: &Graph, g: GateId) -> bool {
    let gate = graph.gate(g);
    match gate.op {
        Op::ChangeDetector
        | Op::EventLoaded
        | Op::EventCustom { .. }
        | Op::WorldEvent
        | Op::Clock => true,
        Op::BufferTicks | Op::BufferSeconds => gate
            .output("Output")
            .is_some_and(|o| !gate.outputs[o as usize].calls.is_empty()),
        Op::Detector(crate::graph::Detector::EdgeExec)
        | Op::Queue { .. }
        | Op::ContainerWatch
        | Op::Stopwatch
        | Op::Tween => true,
        _ => gate.inputs.iter().any(|i| {
            i.exec_pin
                && i.source
                    .is_some_and(|s| !graph.gate(s.gate).outputs[s.out as usize].exec)
        }),
    }
}

/// A world gate's view of its inputs.
struct Inputs<'a> {
    m: &'a mut Machine,
    g: GateId,
}

impl GateInputs for Inputs<'_> {
    fn gate(&self) -> &Gate {
        self.m.graph.gate(self.g)
    }

    fn input(&mut self, name: &str) -> Result<Option<Value>, SimError> {
        self.m.read_named(self.g, name)
    }
}

/// What a Set Variable without `bChangeType` stores when `v` is of another
/// kind than the var's `cur`: the game converts to the var's kind (P34c). A
/// float into an int var rounds toward zero (2.7 and 2.5 give 2, -2.7 gives
/// -2), a bool into a number is 0 or 1, a number into a bool is its
/// truthiness, and an int into a string var is its text. Other conversions
/// between these kinds are not measured and stop the run; anything else is
/// stored as given.
fn set_in_kind(cur: Option<&Value>, v: Option<Value>) -> Result<Option<Value>, String> {
    let Some(cur) = cur else { return Ok(v) };
    let Some(v) = v else { return Ok(None) };
    if std::mem::discriminant(cur) == std::mem::discriminant(&v) {
        return Ok(Some(v));
    }
    Ok(Some(match (cur, v) {
        (Value::Float(_), Value::Int(n)) => Value::Float(n as f64),
        (Value::Float(_), Value::Bool(b)) => Value::Float(f64::from(u8::from(b))),
        (Value::Int(_), Value::Float(f)) => Value::Int(f.trunc() as i64),
        (Value::Int(_), Value::Bool(b)) => Value::Int(i64::from(b)),
        (Value::Bool(_), v @ (Value::Int(_) | Value::Float(_))) => Value::Bool(v.truthy()),
        (Value::Str(_), v @ Value::Int(_)) => Value::Str(v.render()),
        (
            c @ (Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Str(_)),
            v @ (Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Str(_)),
        ) => {
            return Err(format!(
                "Set Variable converting {} to {} is not measured",
                v.kind(),
                c.kind()
            ));
        }
        (_, v) => v,
    }))
}

/// For each gate `from..`, the first address (save order) among the gates its
/// exec wires connect it to, both ways. Units run in that order before their
/// own: a root whose chain emits into a signal another handler listens on
/// ranks with that handler (P35: without the signals, P16's main runs after
/// `T` as stored; with them, before it, and the `Change` reaction stored
/// between follows main the same tick).
fn network_firsts(graph: &Graph, from: GateId) -> Vec<crate::load::BrickAddr> {
    let n = graph.gates.len() - from as usize;
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for g in 0..n {
        for out in graph.gates[g + from as usize].outputs.iter().filter(|o| o.exec) {
            for call in out.calls.iter().filter(|c| !c.remote && c.gate >= from) {
                let (a, b) = (find(&mut parent, g), find(&mut parent, (call.gate - from) as usize));
                parent[a] = b;
            }
        }
    }
    let mut first: HashMap<usize, crate::load::BrickAddr> = HashMap::default();
    for g in 0..n {
        let root = find(&mut parent, g);
        let addr = graph.gates[g + from as usize].addr;
        first
            .entry(root)
            .and_modify(|a| *a = (*a).min(addr))
            .or_insert(addr);
    }
    (0..n).map(|g| first[&find(&mut parent, g)]).collect()
}
