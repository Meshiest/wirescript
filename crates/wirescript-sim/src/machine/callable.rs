//! Callable planes: a function definition, run by a Function Call, and a loop
//! plane, run by its shell's Exec pin. Either runs its body synchronously,
//! inside the chain that called it, from the plane's `CallableEntry` gate.
//!
//! Every run of a body (one call, one loop iteration) is an invocation. The
//! game allows 100,000 invocations per tick and then aborts the callable with
//! code 7; the simulator aborts at the invocation that would pass the cap.

use brdb::schema::BrdbValue;

use super::Machine;
use crate::SimError;
use crate::collections::{HashMap, HashSet};
use crate::graph::{GateId, Graph, LoopKind, Op};
use crate::value::Value;

/// Invocations the game runs per tick before it aborts a callable.
pub(super) const MAX_INVOCATIONS_PER_TICK: u64 = 100_000;
/// Callables nested inside one another. The game's `MaxCallableDepth` is
/// server-configured; this bound only stops a runaway recursion.
const MAX_DEPTH: u32 = 256;

/// A callable plane: where its body starts, and its parameter and result
/// gates by port order.
#[derive(Clone, Debug)]
pub(super) struct Callable {
    pub(super) entry: GateId,
    pub(super) inputs: Vec<GateId>,
    pub(super) outputs: Vec<GateId>,
}

/// Every callable plane of `graph`, by its shell gate.
pub(super) fn callables(graph: &Graph) -> HashMap<GateId, Callable> {
    let mut by_grid: HashMap<u32, Vec<GateId>> = HashMap::default();
    for (i, g) in graph.gates.iter().enumerate() {
        by_grid.entry(g.addr.grid).or_default().push(i as GateId);
    }
    let mut out = HashMap::default();
    for (i, shell) in graph.gates.iter().enumerate() {
        if !matches!(shell.op, Op::FunctionDefinition | Op::Loop(_)) {
            continue;
        }
        let Some(grid) = graph.chip_grids.get(&shell.addr) else {
            continue;
        };
        let inside = by_grid.get(grid).map(Vec::as_slice).unwrap_or_default();
        let Some(&entry) = inside
            .iter()
            .find(|&&g| graph.gate(g).op == Op::CallableEntry)
        else {
            continue;
        };
        // Assumed for saves no compiler laid out: slots follow `PortOrder`. The
        // game maps them by brick position, which Wirescript's layout makes
        // agree with `PortOrder`.
        let ordered = |op: Op| {
            let mut v: Vec<GateId> = inside
                .iter()
                .copied()
                .filter(|&g| graph.gate(g).op == op)
                .collect();
            v.sort_by_key(|&g| graph.gate(g).field_i64("PortOrder").unwrap_or(0));
            v
        };
        out.insert(
            i as GateId,
            Callable {
                entry,
                inputs: ordered(Op::FunctionInput),
                outputs: ordered(Op::FunctionOutput),
            },
        );
    }
    out
}

/// A Function Call's `InputSources`: per slot, whether the call's own input
/// is used (otherwise the parameter's default). Missing entries use the input.
fn input_sources(v: Option<&BrdbValue>) -> Vec<bool> {
    let items = match v {
        Some(BrdbValue::Array(a) | BrdbValue::FlatArray(a)) => a.as_slice(),
        _ => return Vec::new(),
    };
    items
        .iter()
        .map(|e| match e {
            BrdbValue::Enum(e) => e.value != 0,
            BrdbValue::U8(n) => *n != 0,
            BrdbValue::I32(n) => *n != 0,
            BrdbValue::I64(n) => *n != 0,
            _ => true,
        })
        .collect()
}

impl Machine {
    fn callable(&self, g: GateId, shell: GateId) -> Result<Callable, SimError> {
        self.callables
            .get(&shell)
            .cloned()
            .ok_or_else(|| self.error(g, "the callable plane has no entry gate"))
    }

    /// Count one invocation; `false` when it passes the tick's cap.
    fn begin_invocation(&mut self) -> bool {
        self.invocations += 1;
        self.invocations <= MAX_INVOCATIONS_PER_TICK
    }

    /// Run the body of `c` once, starting from its entry's `Body`.
    fn run_body(&mut self, g: GateId, c: &Callable) -> Result<(), SimError> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error(g, "callables nested too deeply"));
        }
        let Some(body) = self.graph.gate(c.entry).output("Body") else {
            return Ok(());
        };
        self.depth += 1;
        self.planes.push(self.graph.gate(c.entry).addr.grid);
        self.unions_passed.push(HashSet::default());
        let result = self.fire_chain(c.entry, body);
        self.unions_passed.pop();
        self.planes.pop();
        self.depth -= 1;
        result
    }

    pub(super) fn function_call(&mut self, g: GateId) -> Result<Option<u16>, SimError> {
        let shell = self.target(g, "Definition")?;
        let c = self.callable(g, shell)?;
        let sources = input_sources(self.graph.gate(g).field("InputSources"));
        for (k, &param) in c.inputs.iter().enumerate() {
            let v = if sources.get(k).copied().unwrap_or(true) {
                self.read_named(g, &format!("Input{k}"))?
            } else {
                self.resolve(self.graph.gate(param).field_value("DefaultValue"), self.grid_offset_of(param))
            };
            self.set_out_named(param, "Value", v);
        }
        let aborted = !self.begin_invocation();
        if !aborted {
            self.run_body(g, &c)?;
            for (k, &result) in c.outputs.iter().enumerate() {
                let v = self.read_named(result, "Value")?;
                self.set_out_named(g, &format!("Output{k}"), v);
            }
        }
        self.set_out_named(g, "bAborted", Some(Value::Bool(aborted)));
        self.set_out_named(g, "IterationsExecuted", Some(Value::Int(i64::from(!aborted))));
        Ok(self.graph.gate(g).output("Completed"))
    }

    pub(super) fn run_loop(&mut self, g: GateId, kind: LoopKind) -> Result<Option<u16>, SimError> {
        if self.graph.gate(g).field_bool("bRunAcrossTicks") == Some(true) {
            return Err(self.error(g, "a loop that runs across ticks is not modeled"));
        }
        let c = self.callable(g, g)?;
        let (first, step, count) = match kind {
            LoopKind::CountFor => {
                let count = self.read_named(g, "Count")?.and_then(|v| v.as_int()).unwrap_or(0);
                let first = self.read_named(g, "FirstIndex")?.and_then(|v| v.as_int()).unwrap_or(0);
                let step = self.read_named(g, "Step")?.and_then(|v| v.as_int()).unwrap_or(1);
                (first, step, count.max(0))
            }
            _ => (0, 1, 0),
        };
        let array = match kind {
            LoopKind::ForEachArray => Some(self.target(g, "ArrayVarRef")?),
            _ => None,
        };
        let pairs = match kind {
            LoopKind::ForEachMap => {
                let map = self.target(g, "MapVarRef")?;
                Some(
                    self.map_pairs(map)
                        .ok_or_else(|| self.error(g, "MapVarRef does not name a map"))?,
                )
            }
            _ => None,
        };
        let saved_break = std::mem::take(&mut self.break_requested);
        let mut done: i64 = 0;
        let mut aborted = false;
        loop {
            let i = done as usize;
            let more = match kind {
                LoopKind::CountFor => done < count,
                LoopKind::ForEachArray => array
                    .and_then(|a| self.array_len(a))
                    .is_some_and(|n| i < n),
                LoopKind::ForEachMap => pairs.as_ref().is_some_and(|p| i < p.len()),
            };
            if !more {
                break;
            }
            if !self.begin_invocation() {
                aborted = true;
                break;
            }
            match kind {
                LoopKind::CountFor => {
                    let index = first.wrapping_add(done.wrapping_mul(step));
                    self.set_out_named(c.entry, "Index", Some(Value::Int(index)));
                }
                LoopKind::ForEachArray => {
                    let v = array.and_then(|a| self.array_item(a, i));
                    self.set_out_named(c.entry, "Index", Some(Value::Int(done)));
                    self.set_out_named(c.entry, "Value", v);
                }
                LoopKind::ForEachMap => {
                    let (k, v) = pairs.as_ref().map(|p| p[i].clone()).unzip();
                    self.set_out_named(c.entry, "Index", Some(Value::Int(done)));
                    self.set_out_named(c.entry, "Key", k);
                    self.set_out_named(c.entry, "Value", v);
                }
            }
            self.run_body(g, &c)?;
            done += 1;
            if std::mem::take(&mut self.break_requested) {
                break;
            }
        }
        self.break_requested = saved_break;
        self.set_out_named(g, "IterationsExecuted", Some(Value::Int(done)));
        self.set_out_named(g, "bAborted", Some(Value::Bool(aborted)));
        self.set_out_named(g, "bRunning", Some(Value::Bool(false)));
        Ok(self.graph.gate(g).output("Completed"))
    }
}
