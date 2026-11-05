//! The simulated gate graph: one gate per component, its ports from the
//! game's inventory, and its wires split into value edges and exec calls.
//!
//! An output is an exec output when the inventory types it `exec`, or when it
//! forwards one (a rerouter, a chip pin, a buffer whose input is an exec
//! signal). A wire from an exec output is a call: the target runs when the
//! source fires. Any other wire carries a value. A value wired into an Exec
//! pin is a trigger: the gate runs when that value changes to truthy.

use brdb::schema::{BrdbStruct, BrdbValue, WireValue};
use wirescript_ir::catalog::{RawPortType, default_catalog};
use wirescript_ir::{NodeId, SourceRange};

use crate::collections::HashMap;
use crate::load::{BrickAddr, LoadError, PortAddr, RawSave};
use crate::value::Value;

pub type GateId = u32;

/// What a gate does, decided once from its class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// A pure gate evaluated by the certified table.
    Eval,
    FormatText,
    Concat,
    Select,
    /// A rerouter or chip pin: its output is its input.
    Reroute,
    DeltaTime,
    ServerUptime,
    Var,
    ArrayVar,
    MapVar,
    ChangeDetector,
    BufferTicks,
    BufferSeconds,
    Clock,
    EventLoaded,
    EventCustom { global: bool },
    /// An event the world raises (a chat command, a player joining).
    WorldEvent,
    VarGet,
    VarSet,
    VarIncrement,
    Branch,
    Union,
    BroadcastChat,
    SendEvent { global: bool },
    Array(ArrayOp),
    Map(MapOp),
    /// A microchip shell brick; it does nothing itself.
    Chip,
    FunctionCall,
    /// A function plane's shell; its `Definition` output names it.
    FunctionDefinition,
    /// A loop plane's shell, run by its `Exec` pin.
    Loop(LoopKind),
    /// The gate inside a callable plane whose `Body` starts each run.
    CallableEntry,
    FunctionInput,
    FunctionOutput,
    CallableBreak,
    EnumToInteger,
    IntegerToEnum,
    /// A pure gate whose law no probe has measured yet (`assumed.rs`).
    Assumed(crate::assumed::Law),
    /// A gate that pulses when its input changes.
    Detector(Detector),
    /// Each `ExecIn` records the data inputs, released in order after a
    /// delay.
    Queue { seconds: bool },
    Random,
    Toggle,
    Cycle,
    PrintToConsole,
    /// The current Unix time in whole seconds; a time source.
    UnixEpoch,
    /// Fires `OnChanged` on the tick after its array or map is written.
    ContainerWatch,
    Stopwatch,
    Tween,
    Dampen,
    /// An asset reference brick: its `Value` names the asset.
    AssetRef,
    /// A component whose outputs the world supplies each tick (a seat
    /// control splitter reading its character's inputs).
    WorldValue,
    /// Spawns a copy of its saved prefab on `Exec`.
    PrefabSpawner,
    /// Destroys the spawned copy its `Entity` names.
    DestroyPrefab,
    /// A gate of a destroyed copy: it does nothing.
    Dead,
    /// Anything the simulator does not model.
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detector {
    /// `bPulseOnRisingEdge` / `bPulseOnFallingEdge` for one tick as a number
    /// rises or falls.
    Edge,
    /// `bPulseOnChange` for one tick on any change.
    Change,
    /// Fires `OnRisingEdge` / `OnFallingEdge` as a number rises or falls.
    EdgeExec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopKind {
    CountFor,
    ForEachArray,
    ForEachMap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayOp {
    Shuffle,
    Push,
    Pop,
    Get,
    SetAtIndex,
    GetLength,
    Clear,
    Insert,
    RemoveAtIndex,
    Find,
    Sum,
    Fill,
    Resize,
    Reverse,
    Swap,
    Join,
    Min,
    Max,
    Average,
    Append,
    CopyFrom,
    Slice,
    Sort,
    SortMultiple,
}

impl ArrayOp {
    /// Whether the op writes its array.
    pub fn writes(self) -> bool {
        !matches!(
            self,
            Self::Get | Self::GetLength | Self::Find | Self::Sum | Self::Join | Self::Min | Self::Max | Self::Average
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapOp {
    Get,
    Set,
    Has,
    Remove,
    Clear,
    GetLength,
    GetKeys,
    GetValues,
    CopyFrom,
}

impl Op {
    pub fn of(class: &str) -> Self {
        let short = class
            .strip_prefix("BrickComponentType_")
            .or_else(|| class.strip_prefix("Component_"))
            .unwrap_or(class);
        if let Some(law) = crate::assumed::Law::of(short) {
            return Self::Assumed(law);
        }
        match short {
            "Expr_String_FormatText" => Self::FormatText,
            "Expr_String_Concatenate" => Self::Concat,
            "Expr_Select" => Self::Select,
            "Internal_Rerouter" | "Internal_MicrochipInput" | "Internal_MicrochipOutput" => {
                Self::Reroute
            }
            "Expr_DeltaTime" => Self::DeltaTime,
            "Expr_ServerUptime" => Self::ServerUptime,
            "Var" => Self::Var,
            "ArrayVar" => Self::ArrayVar,
            "MapVar" => Self::MapVar,
            "Expr_ChangeDetectorExec" => Self::ChangeDetector,
            "Expr_EdgeDetector" => Self::Detector(Detector::Edge),
            "Expr_ChangeDetector" => Self::Detector(Detector::Change),
            "Expr_EdgeDetectorExec" => Self::Detector(Detector::EdgeExec),
            "QueueTicks" => Self::Queue { seconds: false },
            "Exec_Random" => Self::Random,
            "Exec_Toggle" => Self::Toggle,
            "Exec_Cycle" => Self::Cycle,
            "Exec_PrintToConsole" => Self::PrintToConsole,
            "Expr_GetUnixEpoch" => Self::UnixEpoch,
            "Expr_ContainerChangeDetector" => Self::ContainerWatch,
            "Stopwatch" => Self::Stopwatch,
            "Tween" => Self::Tween,
            "Dampen" => Self::Dampen,
            "Exec_PrefabSpawner" => Self::PrefabSpawner,
            "Exec_Entity_DestroySpawnedPrefab" => Self::DestroyPrefab,
            "Internal_InputSplitter"
            | "Expr_Gamemode_IsBuilderTeam"
            | "Expr_Gamemode_IsUnaffiliatedTeam" => Self::WorldValue,
            s if s.ends_with("Reference") && !s.starts_with("Expr_") => Self::AssetRef,
            "QueueSeconds" => Self::Queue { seconds: true },
            "BufferTicks" => Self::BufferTicks,
            "BufferSeconds" => Self::BufferSeconds,
            "Clock" => Self::Clock,
            "Event_Loaded" => Self::EventLoaded,
            "Event_Custom" => Self::EventCustom { global: false },
            "Event_CustomGlobal" => Self::EventCustom { global: true },
            s if s.starts_with("Event_") => Self::WorldEvent,
            "Exec_Var_Get" => Self::VarGet,
            "Exec_Var_Set" => Self::VarSet,
            "Exec_Var_Increment" => Self::VarIncrement,
            "Exec_Branch" => Self::Branch,
            "Exec_Union" => Self::Union,
            "Exec_Gamemode_BroadcastChatMessage" => Self::BroadcastChat,
            "Exec_SendCustomEvent" => Self::SendEvent { global: false },
            "Exec_SendCustomEventGlobal" => Self::SendEvent { global: true },
            "Internal_Microchip" => Self::Chip,
            "FunctionCall" => Self::FunctionCall,
            "Internal_Microchip_FunctionDefinition" => Self::FunctionDefinition,
            "Internal_Microchip_CountFor" => Self::Loop(LoopKind::CountFor),
            "Internal_Microchip_ForEachArray" => Self::Loop(LoopKind::ForEachArray),
            "Internal_Microchip_ForEachMap" => Self::Loop(LoopKind::ForEachMap),
            "CallableEntry_Function"
            | "CallableEntry_CountFor"
            | "CallableEntry_ForEachArray"
            | "CallableEntry_ForEachMap" => Self::CallableEntry,
            "FunctionInput" => Self::FunctionInput,
            "FunctionOutput" => Self::FunctionOutput,
            "Exec_CallableBreak" => Self::CallableBreak,
            "Expr_EnumToInteger" => Self::EnumToInteger,
            "Expr_IntegerToEnum" => Self::IntegerToEnum,
            s if s.starts_with("Exec_ArrayVar_") => match &s["Exec_ArrayVar_".len()..] {
                "Push" => Self::Array(ArrayOp::Push),
                "Pop" => Self::Array(ArrayOp::Pop),
                "Get" => Self::Array(ArrayOp::Get),
                "SetAtIndex" => Self::Array(ArrayOp::SetAtIndex),
                "GetLength" => Self::Array(ArrayOp::GetLength),
                "Clear" => Self::Array(ArrayOp::Clear),
                "Insert" => Self::Array(ArrayOp::Insert),
                "RemoveAtIndex" => Self::Array(ArrayOp::RemoveAtIndex),
                "Find" => Self::Array(ArrayOp::Find),
                "Sum" => Self::Array(ArrayOp::Sum),
                "Fill" => Self::Array(ArrayOp::Fill),
                "Resize" => Self::Array(ArrayOp::Resize),
                "Reverse" => Self::Array(ArrayOp::Reverse),
                "Swap" => Self::Array(ArrayOp::Swap),
                "Join" => Self::Array(ArrayOp::Join),
                "Min" => Self::Array(ArrayOp::Min),
                "Max" => Self::Array(ArrayOp::Max),
                "Average" => Self::Array(ArrayOp::Average),
                "Append" => Self::Array(ArrayOp::Append),
                "CopyFrom" => Self::Array(ArrayOp::CopyFrom),
                "Slice" => Self::Array(ArrayOp::Slice),
                "Sort" => Self::Array(ArrayOp::Sort),
                "Shuffle" => Self::Array(ArrayOp::Shuffle),
                "SortMultiple" => Self::Array(ArrayOp::SortMultiple),
                _ => Self::Unsupported,
            },
            s if s.starts_with("Exec_MapVar_") => match &s["Exec_MapVar_".len()..] {
                "Get" => Self::Map(MapOp::Get),
                "Set" => Self::Map(MapOp::Set),
                "Has" => Self::Map(MapOp::Has),
                "Remove" => Self::Map(MapOp::Remove),
                "Clear" => Self::Map(MapOp::Clear),
                "GetLength" => Self::Map(MapOp::GetLength),
                "GetKeys" => Self::Map(MapOp::GetKeys),
                "GetValues" => Self::Map(MapOp::GetValues),
                "CopyFrom" => Self::Map(MapOp::CopyFrom),
                _ => Self::Unsupported,
            },
            s if s.starts_with("Expr_") && wirescript_ir::semantics::sim_certifies(class) =>
            {
                Self::Eval
            }
            _ => Self::Unsupported,
        }
    }

    /// Evaluated on demand from its inputs, with no state of its own.
    pub fn is_pure(self) -> bool {
        matches!(
            self,
            Self::Eval
                | Self::FormatText
                | Self::Concat
                | Self::Select
                | Self::Reroute
                | Self::DeltaTime
                | Self::ServerUptime
                | Self::EnumToInteger
                | Self::IntegerToEnum
                | Self::Assumed(_)
                | Self::UnixEpoch
                | Self::AssetRef
                | Self::WorldValue
        )
    }
}

/// Where a wire into an input comes from.
#[derive(Clone, Copy, Debug)]
pub struct Src {
    pub gate: GateId,
    pub out: u16,
    /// The wire crosses grids.
    pub remote: bool,
}

#[derive(Clone)]
pub struct Input {
    pub name: String,
    /// The inventory types this port `exec`.
    pub exec_pin: bool,
    /// The value the save stores in the port's field, or `None` when the
    /// field holds the schema default (an unwired input).
    pub stored: Option<Value>,
    pub source: Option<Src>,
}

/// An exec call: the gate and input a fired exec output runs.
#[derive(Clone, Copy, Debug)]
pub struct Call {
    pub gate: GateId,
    pub input: u16,
    pub remote: bool,
}

#[derive(Clone)]
pub struct Output {
    pub name: String,
    /// Fires calls rather than carrying a value.
    pub exec: bool,
    /// Value edges, in stored wire order.
    pub readers: Vec<(GateId, u16)>,
    /// Exec edges, in stored wire order.
    pub calls: Vec<Call>,
    /// What the output holds before anything sets it, when the save stores
    /// nothing for it: the default of its inventory type.
    pub default: Option<Value>,
}

/// The value a port of inventory type `ty` starts with.
fn type_default(ty: RawPortType) -> Option<Value> {
    Some(match ty {
        RawPortType::Bool => Value::Bool(false),
        RawPortType::Int => Value::Int(0),
        RawPortType::Float => Value::Float(0.0),
        RawPortType::String => Value::Str(String::new()),
        RawPortType::Vector => Value::Vector { x: 0.0, y: 0.0, z: 0.0 },
        RawPortType::Rotator => Value::Rotator { pitch: 0.0, yaw: 0.0, roll: 0.0 },
        RawPortType::Color => Value::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
        RawPortType::Entity | RawPortType::Character | RawPortType::Controller => Value::Object(0),
        _ => return None,
    })
}

#[derive(Clone)]
pub struct Gate {
    pub class: String,
    pub op: Op,
    pub addr: BrickAddr,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub data: Option<BrdbStruct>,
    /// The IR node this gate was emitted from, when the graph came from a
    /// Wirescript program.
    pub node: Option<NodeId>,
    pub span: Option<SourceRange>,
}

impl Gate {
    /// A gate of `class` with the inventory's ports, nothing wired and
    /// nothing stored, at `addr`.
    pub fn new(class: &str, addr: BrickAddr) -> Self {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        if let Some(spec) = default_catalog().find_by_class(class) {
            for p in &spec.component.inputs {
                inputs.push(Input {
                    name: p.name.clone(),
                    exec_pin: p.ty == RawPortType::Exec,
                    stored: None,
                    source: None,
                });
            }
            for p in &spec.component.outputs {
                outputs.push(Output {
                    name: p.name.clone(),
                    exec: p.ty == RawPortType::Exec || fires_as_exec(class, &p.name),
                    readers: Vec::new(),
                    calls: Vec::new(),
                    default: type_default(p.ty),
                });
            }
        }
        Self {
            class: class.to_string(),
            op: Op::of(class),
            addr,
            inputs,
            outputs,
            data: None,
            node: None,
            span: None,
        }
    }

    pub fn input(&self, name: &str) -> Option<u16> {
        self.inputs.iter().position(|p| p.name == name).map(|i| i as u16)
    }

    pub fn output(&self, name: &str) -> Option<u16> {
        self.outputs.iter().position(|p| p.name == name).map(|i| i as u16)
    }

    /// A config field of the gate's data struct.
    pub fn field(&self, name: &str) -> Option<&BrdbValue> {
        self.data.as_ref()?.get(name)
    }

    pub fn field_str(&self, name: &str) -> Option<String> {
        match self.field(name)? {
            BrdbValue::String(s) => Some(s.clone()),
            v => match WireValue::try_from(v).ok()? {
                WireValue::Str(s) => Some(s),
                _ => struct_string(v),
            },
        }
    }

    pub fn field_f64(&self, name: &str) -> Option<f64> {
        match self.field(name)? {
            BrdbValue::F32(f) => Some(f64::from(*f)),
            BrdbValue::F64(f) => Some(*f),
            BrdbValue::I32(n) => Some(f64::from(*n)),
            BrdbValue::I64(n) => Some(*n as f64),
            v => Value::from_wire(&WireValue::try_from(v).ok()?)?.as_float(),
        }
    }

    pub fn field_i64(&self, name: &str) -> Option<i64> {
        match self.field(name)? {
            BrdbValue::I32(n) => Some(i64::from(*n)),
            BrdbValue::I64(n) => Some(*n),
            BrdbValue::U8(n) => Some(i64::from(*n)),
            BrdbValue::U32(n) => Some(i64::from(*n)),
            v => Value::from_wire(&WireValue::try_from(v).ok()?)?.as_int(),
        }
    }

    pub fn field_bool(&self, name: &str) -> Option<bool> {
        match self.field(name)? {
            BrdbValue::Bool(b) => Some(*b),
            v => Value::from_wire(&WireValue::try_from(v).ok()?).map(|v| v.truthy()),
        }
    }

    /// A field holding a `WireValue`.
    pub fn field_value(&self, name: &str) -> Option<Value> {
        Value::from_wire(&WireValue::try_from(self.field(name)?).ok()?)
    }
}

/// An output the inventory types as a value that also fires as an exec
/// signal: Whole Grid Interacted's `Character` runs what it is wired to on every
/// interaction, not only when the character changes.
fn fires_as_exec(class: &str, output: &str) -> bool {
    class.ends_with("Event_WholeGridInteracted") && output == "Character"
}

/// A struct field that wraps one string (an event name struct).
fn struct_string(v: &BrdbValue) -> Option<String> {
    let BrdbValue::Struct(s) = v else { return None };
    s.properties.values().find_map(|p| match p {
        BrdbValue::String(s) => Some(s.clone()),
        _ => None,
    })
}

#[derive(Clone)]
pub struct Graph {
    pub gates: Vec<Gate>,
    pub by_addr: HashMap<(BrickAddr, u16), GateId>,
    /// Microchip brick -> its inner grid.
    pub chip_grids: HashMap<BrickAddr, u32>,
    /// The prefabs spawners name, by path.
    pub prefabs: HashMap<String, Vec<u8>>,
}

impl Graph {
    pub fn gate(&self, id: GateId) -> &Gate {
        &self.gates[id as usize]
    }

    /// Gate `id` as an error message names it: its class and, when known, the
    /// source line it came from.
    pub fn describe(&self, id: GateId) -> String {
        let g = self.gate(id);
        let short = g
            .class
            .strip_prefix("BrickComponentType_")
            .unwrap_or(&g.class);
        match &g.span {
            Some(r) => {
                let file = std::path::Path::new(&*r.file)
                    .file_name()
                    .map_or_else(|| r.file.to_string(), |f| f.to_string_lossy().into_owned());
                format!("{short} at {file}:{}:{}", r.start.line, r.start.col)
            }
            None => format!(
                "{short} (grid {} brick {})",
                g.addr.grid, g.addr.brick
            ),
        }
    }

    /// A graph of unwired gates, each stored at its own address.
    pub fn from_gates(gates: Vec<Gate>) -> Self {
        let by_addr = gates
            .iter()
            .enumerate()
            .map(|(i, g)| ((g.addr, 0), i as GateId))
            .collect();
        Self {
            gates,
            by_addr,
            chip_grids: HashMap::default(),
            prefabs: HashMap::default(),
        }
    }

    /// Add `copy`'s gates after these, its grids renumbered past every grid
    /// here so addresses stay unique. Returns how far its grids moved.
    pub fn append(&mut self, copy: Self) -> u32 {
        let base = self.gates.len() as GateId;
        let grids = self.gates.iter().map(|g| g.addr.grid).chain(self.chip_grids.values().copied());
        let offset = grids.max().map_or(0, |m| m + 1);
        let moved = |a: BrickAddr| BrickAddr { grid: a.grid + offset, ..a };
        for mut gate in copy.gates {
            gate.addr = moved(gate.addr);
            for input in &mut gate.inputs {
                if let Some(src) = &mut input.source {
                    src.gate += base;
                }
            }
            for output in &mut gate.outputs {
                for r in &mut output.readers {
                    r.0 += base;
                }
                for c in &mut output.calls {
                    c.gate += base;
                }
            }
            self.gates.push(gate);
        }
        for ((addr, ty), g) in copy.by_addr {
            self.by_addr.insert((moved(addr), ty), g + base);
        }
        for (addr, grid) in copy.chip_grids {
            self.chip_grids.insert(moved(addr), grid + offset);
        }
        self.prefabs.extend(copy.prefabs);
        offset
    }

    pub fn build(save: RawSave) -> Result<Self, LoadError> {
        let mut gates = Vec::with_capacity(save.components.len());
        let mut by_addr = HashMap::default();
        for c in save.components {
            let id = gates.len() as GateId;
            if by_addr.insert((c.addr, c.type_index), id).is_some() {
                return Err(LoadError::Malformed(format!(
                    "two {} components on one brick ({:?})",
                    c.class, c.addr
                )));
            }
            let mut gate = Gate::new(&c.class, c.addr);
            if let Some(data) = &c.data {
                for input in &mut gate.inputs {
                    input.stored = data.get(&input.name).and_then(stored_value);
                }
            }
            gate.data = c.data;
            gates.push(gate);
        }

        let mut graph = Self {
            gates,
            by_addr,
            chip_grids: save.chip_grids,
            prefabs: save.prefabs,
        };

        // Attach every wire to its ports, creating ports the inventory does
        // not list (a gate class it lacks).
        let mut edges = Vec::with_capacity(save.wires.len());
        for w in &save.wires {
            let src = graph.resolve(w.source, &save.port_names, true)?;
            let tgt = graph.resolve(w.target, &save.port_names, false)?;
            let input = &mut graph.gates[tgt.0 as usize].inputs[tgt.1 as usize];
            if input.source.is_some() {
                return Err(LoadError::Malformed(format!(
                    "two wires into {}.{}",
                    graph.describe(tgt.0),
                    graph.gates[tgt.0 as usize].inputs[tgt.1 as usize].name
                )));
            }
            input.source = Some(Src {
                gate: src.0,
                out: src.1,
                remote: w.remote,
            });
            edges.push((src, tgt, w.remote));
        }

        let exec_out = graph.exec_outputs();
        for ((sg, so), (tg, ti), remote) in edges {
            if exec_out[&(sg, so)] {
                graph.gates[sg as usize].outputs[so as usize].calls.push(Call {
                    gate: tg,
                    input: ti,
                    remote,
                });
            } else {
                graph.gates[sg as usize].outputs[so as usize]
                    .readers
                    .push((tg, ti));
            }
        }
        for ((g, o), exec) in exec_out {
            graph.gates[g as usize].outputs[o as usize].exec = exec;
        }
        Ok(graph)
    }

    /// The gate and port index a wire end names.
    fn resolve(
        &mut self,
        end: PortAddr,
        port_names: &[String],
        output: bool,
    ) -> Result<(GateId, u16), LoadError> {
        let gate = *self
            .by_addr
            .get(&(end.brick, end.type_index))
            .ok_or_else(|| {
                LoadError::Malformed(format!(
                    "a wire names component type {} on a brick without one ({:?})",
                    end.type_index, end.brick
                ))
            })?;
        let name = port_names
            .get(end.port as usize)
            .ok_or_else(|| LoadError::Malformed(format!("wire port index {}", end.port)))?;
        let g = &mut self.gates[gate as usize];
        let wrong_side = if output {
            g.output(name).is_none() && g.input(name).is_some()
        } else {
            g.input(name).is_none() && g.output(name).is_some()
        };
        if wrong_side {
            return Err(LoadError::Malformed(format!(
                "a wire {} {}.{}, which is an {} port",
                if output { "leaves" } else { "enters" },
                g.class,
                name,
                if output { "input" } else { "output" }
            )));
        }
        let found = if output {
            g.output(name)
        } else {
            g.input(name)
        };
        let idx = match found {
            Some(i) => i,
            None if output => {
                g.outputs.push(Output {
                    name: name.clone(),
                    exec: false,
                    readers: Vec::new(),
                    calls: Vec::new(),
                    default: None,
                });
                (g.outputs.len() - 1) as u16
            }
            None => {
                let stored = g.data.as_ref().and_then(|d| d.get(name)).and_then(stored_value);
                g.inputs.push(Input {
                    name: name.clone(),
                    exec_pin: false,
                    stored,
                    source: None,
                });
                (g.inputs.len() - 1) as u16
            }
        };
        Ok((gate, idx))
    }

    /// Whether each wired-from output fires calls. A forwarding gate (a
    /// rerouter, a chip pin, a buffer) carries whatever its input carries.
    fn exec_outputs(&self) -> HashMap<(GateId, u16), bool> {
        let mut memo: HashMap<(GateId, u16), bool> = HashMap::default();
        for (gid, g) in self.gates.iter().enumerate() {
            for o in 0..g.outputs.len() {
                self.out_is_exec(gid as GateId, o as u16, &mut memo);
            }
        }
        memo
    }

    /// Follow forwarding gates back from output `o` of `g` to the output
    /// that decides it. A forwarding loop carries no exec signal.
    fn out_is_exec(&self, g: GateId, o: u16, memo: &mut HashMap<(GateId, u16), bool>) -> bool {
        let mut path: Vec<(GateId, u16)> = Vec::new();
        let mut on_path: crate::collections::HashSet<(GateId, u16)> =
            crate::collections::HashSet::default();
        let (mut g, mut o) = (g, o);
        let exec = loop {
            if let Some(&e) = memo.get(&(g, o)) {
                break e;
            }
            if !on_path.insert((g, o)) {
                break false;
            }
            path.push((g, o));
            let gate = self.gate(g);
            let out = &gate.outputs[o as usize];
            let forwarded = match gate.op {
                Op::Reroute => Some("RER_Input"),
                Op::BufferTicks | Op::BufferSeconds if out.name == "Output" => Some("Input"),
                _ => None,
            };
            let Some(input) = forwarded else {
                break out.exec;
            };
            match gate.input(input).and_then(|i| gate.inputs[i as usize].source) {
                Some(s) => (g, o) = (s.gate, s.out),
                None => break false,
            }
        };
        for key in path {
            memo.insert(key, exec);
        }
        exec
    }
}

/// The value an input field stores, or `None` for the schema default of a
/// `WireValue` field (`Number(0.0)`), which is how an unwired input is saved,
/// and for a field holding no wire value (an exec marker, an object).
fn stored_value(v: &BrdbValue) -> Option<Value> {
    if let Ok(WireValue::Number(f)) = WireValue::try_from(v)
        && f == 0.0
        && f.is_sign_positive()
    {
        return None;
    }
    Value::from_brdb(v)
}
