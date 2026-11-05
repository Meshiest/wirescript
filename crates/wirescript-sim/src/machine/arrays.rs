//! Array and map storage and the exec gates that work on them.
//!
//! An array keeps its declared element type (the save's `dynamic_array` type
//! name); a value stored into it takes that type. Out-of-range reads leave the
//! gate's `Value` output as it was, which is how the game keeps "the previous
//! value".

use brdb::schema::BrdbValue;

use super::{Machine, State};
use crate::SimError;
use crate::collections::HashMap;
use crate::graph::{ArrayOp, GateId, MapOp};
use crate::value::Value;

#[derive(Clone, Debug, Default)]
pub(super) struct ArrayStore {
    pub(super) items: Vec<Value>,
    /// The element type, as the save names it (`i64`, `f64`, `Str`, ...).
    pub(super) elem: String,
}

/// A map variable: pairs in insertion order, removed slots kept dead.
#[derive(Clone, Debug, Default)]
pub(super) struct MapStore {
    pub(super) slots: Vec<Option<(Value, Value)>>,
    pub(super) index: HashMap<MapKey, usize>,
    pub(super) value_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum MapKey {
    Int(i64),
    Float(u64),
    Bool(bool),
    Str(String),
    Other(String),
}

/// The order Sort puts elements in: text by text, everything else as a
/// number.
fn sort_order(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        _ => a.as_float().unwrap_or(0.0).total_cmp(&b.as_float().unwrap_or(0.0)),
    }
}

pub(super) fn map_key(v: &Value) -> MapKey {
    match v {
        Value::Int(n) => MapKey::Int(*n),
        Value::Float(f) => MapKey::Float(f.to_bits()),
        Value::Bool(b) => MapKey::Bool(*b),
        Value::Str(s) => MapKey::Str(s.clone()),
        other => MapKey::Other(format!("{other:?}")),
    }
}

/// The initial contents an `ArrayVar` stores.
pub(super) fn array_from_field(v: Option<&BrdbValue>) -> ArrayStore {
    match v {
        Some(BrdbValue::DynamicArray(a)) => ArrayStore {
            items: a.values.iter().filter_map(Value::from_brdb).collect(),
            elem: a.element_type.clone(),
        },
        _ => ArrayStore {
            items: Vec::new(),
            elem: "f64".into(),
        },
    }
}

/// The initial contents a `MapVar` stores.
pub(super) fn map_from_field(v: Option<&BrdbValue>) -> MapStore {
    let mut m = MapStore {
        value_type: "f64".into(),
        ..MapStore::default()
    };
    if let Some(BrdbValue::DynamicMap(d)) = v {
        m.value_type.clone_from(&d.value_type);
        for (k, v) in &d.entries {
            if let (Some(k), Some(v)) = (Value::from_brdb(k), Value::from_brdb(v)) {
                m.index.insert(map_key(&k), m.slots.len());
                m.slots.push(Some((k, v)));
            }
        }
    }
    m
}

/// The default element of type `elem`.
fn default_of(elem: &str) -> Value {
    match elem {
        "i64" => Value::Int(0),
        "Bool" => Value::Bool(false),
        "Str" => Value::Str(String::new()),
        "Vector" => Value::Vector { x: 0.0, y: 0.0, z: 0.0 },
        "Rotator" => Value::Rotator { pitch: 0.0, yaw: 0.0, roll: 0.0 },
        "Quat" => Value::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
        "LinearColor" => Value::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
        "weak_object" => Value::Object(0),
        _ => Value::Float(0.0),
    }
}

/// `v` stored into a slot of type `elem`. Assumed: numbers convert between
/// int and float (truncating), anything else is kept as it is.
fn coerce(v: Option<Value>, elem: &str) -> Value {
    let Some(v) = v else {
        return default_of(elem);
    };
    match (elem, &v) {
        ("i64", Value::Float(_) | Value::Bool(_)) => Value::Int(v.as_int().unwrap_or(0)),
        ("f64", Value::Int(_) | Value::Bool(_)) => Value::Float(v.as_float().unwrap_or(0.0)),
        ("Bool", Value::Int(_) | Value::Float(_) | Value::Str(_)) => Value::Bool(v.truthy()),
        _ => v,
    }
}

impl Machine {
    fn array_mut(&mut self, g: GateId, port: &str) -> Result<GateId, SimError> {
        let arr = self.target(g, port)?;
        if matches!(self.state[arr as usize], State::Array(_)) {
            Ok(arr)
        } else {
            Err(self.error(g, &format!("{port} does not name an array")))
        }
    }

    fn items(&mut self, arr: GateId) -> &mut ArrayStore {
        match &mut self.state[arr as usize] {
            State::Array(a) => a,
            _ => unreachable!("checked by array_mut"),
        }
    }

    fn int_input(&mut self, g: GateId, port: &str) -> Result<i64, SimError> {
        Ok(self.read_named(g, port)?.and_then(|v| v.as_int()).unwrap_or(0))
    }

    /// The sum, minimum or maximum of `items` by the certified math and
    /// compare laws.
    fn fold_items(&self, g: GateId, items: &[Value], class: &str) -> Result<Option<Value>, SimError> {
        let mut acc: Option<Value> = None;
        for v in items {
            acc = Some(match acc {
                None => v.clone(),
                Some(a) => match class {
                    "Sum" => self
                        .eval_class(g, "BrickComponentType_Expr_MathAdd", &[Some(a), Some(v.clone())])?
                        .unwrap_or_default(),
                    _ => {
                        let less = self.eval_class(
                            g,
                            "BrickComponentType_Expr_CompareLess",
                            &[Some(v.clone()), Some(a.clone())],
                        )?;
                        let take = less.is_some_and(|l| l.truthy()) == (class == "Min");
                        if take { v.clone() } else { a }
                    }
                },
            });
        }
        Ok(acc)
    }

    pub(super) fn array_op(&mut self, g: GateId, op: ArrayOp) -> Result<(), SimError> {
        let arr = self.array_mut(g, "ArrayVarRef")?;
        let mut outs: Vec<(&str, Option<Value>)> = Vec::new();
        let in_bounds = |i: i64, len: usize| i >= 0 && (i as usize) < len;
        match op {
            ArrayOp::Push => {
                let v = self.read_named(g, "Value")?;
                let a = self.items(arr);
                let v = coerce(v, &a.elem);
                a.items.push(v);
            }
            ArrayOp::Pop => {
                let v = self.items(arr).items.pop();
                outs.push(("bIsEmpty", Some(Value::Bool(v.is_none()))));
                if let Some(v) = v {
                    outs.push(("Value", Some(v)));
                }
            }
            ArrayOp::Get => {
                let i = self.int_input(g, "Index")?;
                let a = self.items(arr);
                let oob = !in_bounds(i, a.items.len());
                let v = (!oob).then(|| a.items[i as usize].clone());
                outs.push(("bOutOfBounds", Some(Value::Bool(oob))));
                if let Some(v) = v {
                    outs.push(("Value", Some(v)));
                }
            }
            ArrayOp::SetAtIndex => {
                let i = self.int_input(g, "Index")?;
                let v = self.read_named(g, "Value")?;
                let a = self.items(arr);
                if in_bounds(i, a.items.len()) {
                    a.items[i as usize] = coerce(v, &a.elem);
                }
            }
            ArrayOp::GetLength => {
                let n = self.items(arr).items.len();
                outs.push(("Length", Some(Value::Int(n as i64))));
            }
            ArrayOp::Clear => self.items(arr).items.clear(),
            ArrayOp::Insert => {
                let i = self.int_input(g, "Index")?;
                let v = self.read_named(g, "Value")?;
                let a = self.items(arr);
                let ok = i >= 0 && (i as usize) <= a.items.len();
                if ok {
                    let v = coerce(v, &a.elem);
                    a.items.insert(i as usize, v);
                }
                outs.push(("bOutOfBounds", Some(Value::Bool(!ok))));
            }
            ArrayOp::RemoveAtIndex => {
                let i = self.int_input(g, "Index")?;
                let a = self.items(arr);
                let ok = in_bounds(i, a.items.len());
                if ok {
                    a.items.remove(i as usize);
                }
                outs.push(("bOutOfBounds", Some(Value::Bool(!ok))));
            }
            ArrayOp::Find => {
                let v = self.read_named(g, "Value")?;
                let a = self.items(arr);
                let v = coerce(v, &a.elem);
                let found = a.items.iter().position(|x| *x == v);
                outs.push(("bFound", Some(Value::Bool(found.is_some()))));
                outs.push(("Index", Some(Value::Int(found.map_or(-1, |i| i as i64)))));
            }
            ArrayOp::Fill => {
                let v = self.read_named(g, "Value")?;
                let a = self.items(arr);
                let v = coerce(v, &a.elem);
                a.items.fill(v);
            }
            ArrayOp::Resize => {
                let n = self.int_input(g, "Size")?.max(0) as usize;
                let v = self.read_named(g, "Value")?;
                if n > super::MAX_ARRAY_LEN {
                    return Err(self.error(g, &format!("resize to {n} elements")));
                }
                let a = self.items(arr);
                let v = coerce(v, &a.elem);
                a.items.resize(n, v);
            }
            ArrayOp::Reverse => self.items(arr).items.reverse(),
            ArrayOp::Swap => {
                let i = self.int_input(g, "IndexA")?;
                let j = self.int_input(g, "IndexB")?;
                let a = self.items(arr);
                let len = a.items.len();
                let ok = in_bounds(i, len) && in_bounds(j, len);
                if ok {
                    a.items.swap(i as usize, j as usize);
                }
                outs.push(("bOutOfBounds", Some(Value::Bool(!ok))));
            }
            ArrayOp::Shuffle => {
                let mut items = std::mem::take(&mut self.items(arr).items);
                self.rng.shuffle(&mut items);
                self.items(arr).items = items;
            }
            ArrayOp::Sum | ArrayOp::Min | ArrayOp::Max | ArrayOp::Average => {
                let items = self.items(arr).items.clone();
                let elem = self.items(arr).elem.clone();
                if op != ArrayOp::Sum {
                    outs.push(("bIsEmpty", Some(Value::Bool(items.is_empty()))));
                }
                let class = match op {
                    ArrayOp::Min => "Min",
                    ArrayOp::Max => "Max",
                    _ => "Sum",
                };
                let folded = self.fold_items(g, &items, class)?;
                let v = match op {
                    ArrayOp::Sum => Some(folded.unwrap_or_else(|| default_of(&elem))),
                    ArrayOp::Average if !items.is_empty() => folded
                        .and_then(|s| s.as_float())
                        .map(|s| Value::Float(s / items.len() as f64)),
                    _ => folded,
                };
                if let Some(v) = v {
                    outs.push(("Value", Some(v)));
                }
            }
            ArrayOp::Join => {
                let sep = match self.read_named(g, "Separator")? {
                    Some(Value::Str(s)) => s,
                    Some(v) => v.render(),
                    None => String::new(),
                };
                let text: Vec<String> = self
                    .items(arr)
                    .items
                    .iter()
                    .map(|v| match v.to_semantic() {
                        Some(s) => wirescript_ir::semantics::concat_display(&s),
                        None => v.to_string(),
                    })
                    .collect();
                let mut joined = text.join(&sep);
                super::cap_string(&mut joined);
                outs.push(("Output", Some(Value::Str(joined))));
            }
            ArrayOp::Append | ArrayOp::CopyFrom => {
                let src = self.array_mut(g, "SourceRef")?;
                let items = self.items(src).items.clone();
                let a = self.items(arr);
                let elem = a.elem.clone();
                let items = items.into_iter().map(|v| coerce(Some(v), &elem));
                if op == ArrayOp::CopyFrom {
                    a.items = items.collect();
                } else {
                    a.items.extend(items);
                }
            }
            ArrayOp::Slice => {
                let src = self.array_mut(g, "SourceRef")?;
                let start = self.int_input(g, "Start")?;
                let count = self.int_input(g, "Count")?;
                let items = self.items(src).items.clone();
                let ok = start >= 0 && count >= 0 && (start as usize).saturating_add(count as usize) <= items.len();
                if ok {
                    let a = self.items(arr);
                    a.items = items[start as usize..(start + count) as usize].to_vec();
                }
                outs.push(("bOutOfBounds", Some(Value::Bool(!ok))));
            }
            ArrayOp::Sort => {
                let desc = self.read_named(g, "bDescending")?.is_some_and(|v| v.truthy());
                let mut items = std::mem::take(&mut self.items(arr).items);
                let key_err = items.iter().any(|v| {
                    !matches!(v, Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Str(_))
                });
                if key_err {
                    self.items(arr).items = items;
                    return Err(self.error(g, "sorting this element type is not modeled"));
                }
                items.sort_by(sort_order);
                if desc {
                    items.reverse();
                }
                self.items(arr).items = items;
            }
            ArrayOp::SortMultiple => {
                let desc = self.read_named(g, "bDescending")?.is_some_and(|v| v.truthy());
                let len = self.items(arr).items.len();
                let mut companions = Vec::new();
                for k in 2..=8 {
                    if let Some(Value::Ref(c)) = self.read_named(g, &format!("ArrayVarRef{k}"))?
                        && c != arr
                        && matches!(self.state[c as usize], State::Array(_))
                        && !companions.contains(&c)
                    {
                        companions.push(c);
                    }
                }
                let lengths_match = companions.iter().all(|&c| self.items(c).items.len() == len);
                outs.push(("bSuccess", Some(Value::Bool(lengths_match))));
                if lengths_match {
                    let keys = &self.items(arr).items;
                    if keys.iter().any(|v| !matches!(v, Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Str(_))) {
                        return Err(self.error(g, "sorting this element type is not modeled"));
                    }
                    let mut order: Vec<usize> = (0..len).collect();
                    order.sort_by(|&i, &j| sort_order(&keys[i], &keys[j]));
                    if desc {
                        order.reverse();
                    }
                    for c in std::iter::once(arr).chain(companions.iter().copied()) {
                        let old = std::mem::take(&mut self.items(c).items);
                        self.items(c).items = order.iter().map(|&i| old[i].clone()).collect();
                    }
                    for c in companions {
                        self.container_changed(c)?;
                    }
                }
            }
        }
        if self.items(arr).items.len() > super::MAX_ARRAY_LEN {
            return Err(self.error(g, "an array grew past ten million elements"));
        }
        if op.writes() {
            self.container_changed(arr)?;
        }
        for (name, v) in outs {
            self.set_out_named(g, name, v);
        }
        Ok(())
    }

    /// Replace what the array input `port` of `g` references with `items`,
    /// converted to its element type.
    pub(super) fn fill_array(&mut self, g: GateId, port: &str, items: Vec<Value>) -> Result<(), SimError> {
        let arr = self.array_mut(g, port)?;
        let a = self.items(arr);
        let elem = a.elem.clone();
        a.items = items.into_iter().map(|v| coerce(Some(v), &elem)).collect();
        self.container_changed(arr)
    }

    pub(super) fn map_op(&mut self, g: GateId, op: MapOp) -> Result<(), SimError> {
        let map = self.target(g, "MapVarRef")?;
        if !matches!(self.state[map as usize], State::Map(_)) {
            return Err(self.error(g, "MapVarRef does not name a map"));
        }
        let key = match op {
            MapOp::Get | MapOp::Set | MapOp::Has | MapOp::Remove => {
                self.read_named(g, "Key")?.unwrap_or_default()
            }
            _ => Value::default(),
        };
        let value = if op == MapOp::Set {
            self.read_named(g, "Value")?
        } else {
            None
        };
        let mut outs: Vec<(&str, Option<Value>)> = Vec::new();
        let k = map_key(&key);
        // Collections a map writes into an array (`GetKeys`, `GetValues`).
        let mut into_array: Option<Vec<Value>> = None;
        let mut copy_from: Option<GateId> = None;
        if op == MapOp::CopyFrom {
            copy_from = Some(self.target(g, "SourceRef")?);
        }
        let include_dead = op == MapOp::GetKeys
            && self.read_named(g, "bIncludeDead")?.is_some_and(|v| v.truthy());
        let source = copy_from.and_then(|s| match &self.state[s as usize] {
            State::Map(m) => Some(m.clone()),
            _ => None,
        });
        let State::Map(m) = &mut self.state[map as usize] else {
            unreachable!()
        };
        match op {
            MapOp::Get => match m.index.get(&k).and_then(|&i| m.slots[i].as_ref()) {
                Some((_, v)) => {
                    outs.push(("bFound", Some(Value::Bool(true))));
                    outs.push(("Value", Some(v.clone())));
                }
                None => outs.push(("bFound", Some(Value::Bool(false)))),
            },
            MapOp::Set => {
                if m.slots.len() >= super::MAX_ARRAY_LEN {
                    return Err(self.error(g, "a map grew past ten million entries"));
                }
                let value = coerce(value, &m.value_type);
                match m.index.get(&k) {
                    Some(&i) => m.slots[i] = Some((key, value)),
                    None => {
                        m.index.insert(k, m.slots.len());
                        m.slots.push(Some((key, value)));
                    }
                }
            }
            MapOp::Has => outs.push(("bFound", Some(Value::Bool(m.index.contains_key(&k))))),
            MapOp::Remove => {
                let found = m.index.remove(&k);
                if let Some(i) = found {
                    m.slots[i] = None;
                }
                outs.push(("bFound", Some(Value::Bool(found.is_some()))));
            }
            MapOp::Clear => {
                m.slots.clear();
                m.index.clear();
            }
            MapOp::GetLength => outs.push(("Length", Some(Value::Int(m.index.len() as i64)))),
            MapOp::GetKeys => {
                if include_dead {
                    return Err(self.error(g, "GetKeys with dead keys is not modeled"));
                }
                into_array = Some(m.slots.iter().flatten().map(|(k, _)| k.clone()).collect());
            }
            MapOp::GetValues => {
                into_array = Some(m.slots.iter().flatten().map(|(_, v)| v.clone()).collect());
            }
            MapOp::CopyFrom => match source {
                Some(src) => {
                    m.slots = src.slots;
                    m.index = src.index;
                }
                None => return Err(self.error(g, "SourceRef does not name a map")),
            },
        }
        if let Some(items) = into_array {
            let arr = self.array_mut(g, "ArrayVarRef")?;
            let a = self.items(arr);
            let elem = a.elem.clone();
            a.items = items.into_iter().map(|v| coerce(Some(v), &elem)).collect();
            self.container_changed(arr)?;
        }
        if matches!(op, MapOp::Set | MapOp::Remove | MapOp::Clear | MapOp::CopyFrom) {
            self.container_changed(map)?;
        }
        for (name, v) in outs {
            self.set_out_named(g, name, v);
        }
        Ok(())
    }

    /// The elements a for-each loop over array `arr` sees.
    pub(super) fn array_len(&self, arr: GateId) -> Option<usize> {
        match &self.state[arr as usize] {
            State::Array(a) => Some(a.items.len()),
            _ => None,
        }
    }

    pub(super) fn array_item(&self, arr: GateId, i: usize) -> Option<Value> {
        match &self.state[arr as usize] {
            State::Array(a) => a.items.get(i).cloned(),
            _ => None,
        }
    }

    /// The live pairs of map `map`, in insertion order.
    pub(super) fn map_pairs(&self, map: GateId) -> Option<Vec<(Value, Value)>> {
        match &self.state[map as usize] {
            State::Map(m) => Some(m.slots.iter().flatten().cloned().collect()),
            _ => None,
        }
    }
}
