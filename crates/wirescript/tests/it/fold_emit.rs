//! Emit-level tests for constant delivery into named chip inputs.
//!
//! A parent-side `_Literal` wired to a named chip instance's `MicrochipInput`
//! has no emit-time delivery mechanism: literal-source wires are skipped at
//! emit (they normally inline into the target's data), but a `MicrochipInput`
//! is a rerouter with no matching data field, and `partition_anon_chips`'
//! literal-clone arm only covers nodes moved into anonymous chips. Without
//! another path the wire is dropped and the chip input reads 0, for both the
//! folded `P(2 + 2)` shape and the `let k = 4; P(k)` alias shape.
//!
//! These tests compile all the way to an emitted `brdb::World` (IR-level checks
//! cannot see this) and assert the pin receives its value: either exactly one
//! incoming wire from a real gate whose embedded data evaluates to the
//! expected constant, or a value baked onto the real consumer.

use brdb::AsBrdbValue;
use brdb::schema::WireValue;
use wirescript::emit::EmitOptions;
use wirescript::{CompileInput, FoldMode, compile_to_world};

const CHIP_IN: &str = "BrickComponentType_Internal_MicrochipInput";
const MATH_ADD: &str = "BrickComponentType_Expr_MathAdd";
const STRING_CONCAT: &str = "BrickComponentType_Expr_String_Concatenate";
const REROUTER: &str = "Component_Internal_Rerouter";

fn world(src: &str, fold_mode: FoldMode) -> brdb::World {
    let r = compile_to_world(
        CompileInput::new(src, "fold_emit.ws").fold_mode(fold_mode),
        EmitOptions::default(),
    )
    .expect("should compile");
    assert!(
        !r.diagnostics
            .iter()
            .any(|d| matches!(d.severity, wirescript::diagnostic::Severity::Error)),
        "no error diagnostics expected: {:?}",
        r.diagnostics
    );
    r.world
}

fn component_is(c: &dyn brdb::BrdbComponent, ty: &str) -> bool {
    c.component_type()
        .map(|t| t.to_string() == ty)
        .unwrap_or(false)
}

/// The `WireValue` an emitted component field holds, read back through the
/// accessors the brdb writer itself uses. `None` when the field is not a
/// wire value (e.g. a native struct).
fn as_wire_value(v: &dyn AsBrdbValue) -> Option<WireValue> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let f = |name: &str| -> f64 {
        let id = schema
            .intern
            .get(name)
            .unwrap_or_else(|| panic!("{name} interned in max schema"));
        v.as_brdb_struct_prop_value(schema, id, id)
            .and_then(brdb::AsBrdbValue::as_brdb_f64)
            .unwrap_or_else(|e| panic!("wire value component {name}: {e:?}"))
    };
    Some(match v.as_brdb_variant_member()? {
        "i64" => WireValue::Int(v.as_brdb_i64().ok()?),
        "f64" => WireValue::Number(v.as_brdb_f64().ok()?),
        "Bool" => WireValue::Bool(v.as_brdb_bool().ok()?),
        "Str" => WireValue::Str(v.as_brdb_str().ok()?.to_string()),
        "Vector" => WireValue::Vector {
            x: f("X"),
            y: f("Y"),
            z: f("Z"),
        },
        "Rotator" => WireValue::Rotator {
            pitch: f("Pitch"),
            yaw: f("Yaw"),
            roll: f("Roll"),
        },
        "Quat" => WireValue::Quat {
            x: f("X"),
            y: f("Y"),
            z: f("Z"),
            w: f("W"),
        },
        _ => return None,
    })
}

/// The chip child grid's `MicrochipInput` brick id (the repro has exactly
/// one value input pin, `v`).
fn chip_input_brick_id(w: &brdb::World) -> usize {
    let mut ids: Vec<usize> = Vec::new();
    for (_entity, bricks) in &w.grids {
        for b in bricks {
            if b.components
                .iter()
                .any(|c| component_is(c.as_ref(), CHIP_IN))
            {
                ids.push(b.id.expect("pin brick has an id"));
            }
        }
    }
    assert_eq!(
        ids.len(),
        1,
        "expected exactly one MicrochipInput pin brick"
    );
    ids[0]
}

fn find_brick(w: &brdb::World, id: usize) -> &brdb::Brick {
    w.bricks
        .iter()
        .chain(w.grids.iter().flat_map(|(_, bricks)| bricks.iter()))
        .find(|b| b.id == Some(id))
        .unwrap_or_else(|| panic!("no brick with id {id}"))
}

/// Evaluate the emitted `MathAdd` component's embedded operands
/// (`InputA + InputB`): the value the gate outputs for unwired inputs.
fn math_add_input_sum(b: &brdb::Brick) -> i64 {
    let schema = brdb::schemas::bricks_components_schema_max();
    let comp = b
        .components
        .iter()
        .find(|c| component_is(c.as_ref(), MATH_ADD))
        .expect("MathAdd component on the wire's source brick");
    let read = |field: &str| -> i64 {
        let fid = schema
            .intern
            .get(field)
            .unwrap_or_else(|| panic!("{field} interned in max schema"));
        let v = comp
            .as_brdb_struct_prop_value(schema, fid, fid)
            .unwrap_or_else(|e| panic!("{field} present in component data: {e:?}"));
        match as_wire_value(v).expect("wire value operand") {
            WireValue::Int(n) => n,
            WireValue::Number(f) => f as i64,
            other => panic!("unexpected {field} value: {other:?}"),
        }
    };
    read("InputA") + read("InputB")
}

/// The core assertion: the chip's `v` pin is delivered the constant by one
/// incoming wire from a real (non-literal) gate that evaluates to `expected`.
fn assert_pin_delivery(what: &str, src: &str, fold_mode: FoldMode, expected: i64) {
    let w = world(src, fold_mode);
    let pin = chip_input_brick_id(&w);
    let incoming: Vec<&brdb::WireConnection> = w
        .wires
        .iter()
        .filter(|c| c.target.brick_id == pin && c.target.port_name.to_string() == "RER_Input")
        .collect();
    assert_eq!(
        incoming.len(),
        1,
        "{what}: the chip input pin must have exactly one incoming wire \
         (a constant argument was silently dropped at emit); wires: {:?}",
        w.wires
    );
    let source = &incoming[0].source;
    assert_eq!(
        source.component_type.to_string(),
        MATH_ADD,
        "{what}: pin should be fed by a real carrier/expression gate"
    );
    let sum = math_add_input_sum(find_brick(&w, source.brick_id));
    assert_eq!(sum, expected, "{what}: delivered constant value");
    println!(
        "{what}: delivered — brick {} {}({sum}) -> brick {pin} {CHIP_IN}.RER_Input",
        source.brick_id, source.component_type,
    );
}

/// The chip input pin's incoming wire count. Boundary-delivery cleanup drops it
/// to zero once the argument is delivered straight to its real consumer
/// (`live_consumer_fed_by_opaque`/`operand_source_brick` below); with
/// `--no-fold` the pin is the only delivery path, which is
/// `assert_pin_delivery`'s "exactly one".
fn chip_input_incoming_wire_count(w: &brdb::World, pin: usize) -> usize {
    w.wires
        .iter()
        .filter(|c| c.target.brick_id == pin && c.target.port_name.to_string() == "RER_Input")
        .count()
}

/// The real, still-live consumer brick of `component_type` with an incoming
/// wire from a live `Opaque(...)` rerouter brick. The `Opaque` side is the fold
/// pass's barrier and is always a real wire, so this finds the same gate
/// however its other (folded) operand arrives: a live wire, baked component
/// data, or a directly wired carrier (see `operand_source_brick`).
fn live_consumer_fed_by_opaque<'w>(w: &'w brdb::World, component_type: &str) -> &'w brdb::Brick {
    let is_rerouter = |brick_id: usize| -> bool {
        find_brick(w, brick_id)
            .components
            .iter()
            .any(|c| component_is(c.as_ref(), REROUTER))
    };
    let consumer_id = w
        .wires
        .iter()
        .find(|wc| {
            wc.target.component_type.to_string() == component_type
                && is_rerouter(wc.source.brick_id)
        })
        .map(|wc| wc.target.brick_id)
        .unwrap_or_else(|| {
            panic!(
                "expected a {component_type} wired from a live Opaque rerouter; wires: {:?}",
                w.wires
            )
        });
    find_brick(w, consumer_id)
}

/// The brick that carries `port_name`'s value for `consumer`. Boundary-delivery
/// cleanup delivers a folded argument to its real consumer in one of two ways:
/// baked onto the consumer's own component data (ints, floats, bools, vectors,
/// rotators and quats inline as a wire-variant default; see
/// `emit::port_accepts_inline_variant`), or, for strings, which cannot inline,
/// via one small carrier gate wired directly into the consumer. If `port_name`
/// is wired, follow it to the carrier; otherwise the value is baked on
/// `consumer` itself.
fn operand_source_brick<'w>(
    w: &'w brdb::World,
    consumer: &brdb::Brick,
    port_name: &str,
) -> &'w brdb::Brick {
    let consumer_id = consumer.id.expect("consumer brick has an id");
    match w.wires.iter().find(|wc| {
        wc.target.brick_id == consumer_id && wc.target.port_name.to_string() == port_name
    }) {
        Some(wc) => find_brick(w, wc.source.brick_id),
        None => find_brick(w, consumer_id),
    }
}

/// Read a wire-variant field of any type off any component (not scoped to a
/// carrier gate class): the shape a folded operand is baked into when it lands
/// on an ordinary math gate's generic operand field, rather than a
/// `MakeVector`/`MakeQuaternion` carrier's separate X/Y/Z(/W) fields.
fn read_wire_value(b: &brdb::Brick, component_type: &str, field: &str) -> WireValue {
    let schema = brdb::schemas::bricks_components_schema_max();
    let comp = b
        .components
        .iter()
        .find(|c| component_is(c.as_ref(), component_type))
        .unwrap_or_else(|| panic!("{component_type} component on brick {:?}", b.id));
    let fid = schema
        .intern
        .get(field)
        .unwrap_or_else(|| panic!("{field} interned in max schema"));
    as_wire_value(
        comp.as_brdb_struct_prop_value(schema, fid, fid)
            .unwrap_or_else(|e| panic!("{field} present in component data: {e:?}")),
    )
    .expect("wire value operand")
}

fn read_wire_value_int(b: &brdb::Brick, component_type: &str, field: &str) -> i64 {
    match read_wire_value(b, component_type, field) {
        WireValue::Int(n) => n,
        WireValue::Number(f) => f as i64,
        other => panic!("unexpected {field} value: {other:?}"),
    }
}

fn read_wire_value_vector(b: &brdb::Brick, component_type: &str, field: &str) -> (f64, f64, f64) {
    match read_wire_value(b, component_type, field) {
        WireValue::Vector { x, y, z } => (x, y, z),
        other => panic!("unexpected {field} value: {other:?}"),
    }
}

/// Some fields (e.g. `QuatSlerp`'s `InputA`/receiver) embed a baked composite
/// as a nested struct (X/Y/Z/W sub-fields) rather than a tagged
/// `WireValue::Quat`, which a generic math gate's operand slot uses. Try the
/// wire-value reading first, then fall back to the nested-struct shape.
fn read_wire_value_quat(
    b: &brdb::Brick,
    component_type: &str,
    field: &str,
) -> (f64, f64, f64, f64) {
    let schema = brdb::schemas::bricks_components_schema_max();
    let comp = b
        .components
        .iter()
        .find(|c| component_is(c.as_ref(), component_type))
        .unwrap_or_else(|| panic!("{component_type} component on brick {:?}", b.id));
    let fid = schema
        .intern
        .get(field)
        .unwrap_or_else(|| panic!("{field} interned in max schema"));
    let value = comp
        .as_brdb_struct_prop_value(schema, fid, fid)
        .unwrap_or_else(|e| panic!("{field} present in component data: {e:?}"));
    if let Some(WireValue::Quat { x, y, z, w }) = as_wire_value(value) {
        return (x, y, z, w);
    }
    let read_sub = |sub: &str| -> f64 {
        let sub_id = schema
            .intern
            .get(sub)
            .unwrap_or_else(|| panic!("{sub} interned in max schema"));
        let sub_val = value
            .as_brdb_struct_prop_value(schema, sub_id, sub_id)
            .unwrap_or_else(|e| panic!("nested {field}.{sub} present: {e:?}"));
        sub_val
            .as_brdb_f64()
            .or_else(|_| sub_val.as_brdb_f32().map(f64::from))
            .unwrap_or_else(|e| panic!("nested {field}.{sub} numeric: {e:?}"))
    };
    (read_sub("X"), read_sub("Y"), read_sub("Z"), read_sub("W"))
}

/// Read a `String_Concatenate` component's baked `InputA` text (a native `str`
/// field).
fn string_concat_input_a(b: &brdb::Brick) -> String {
    let schema = brdb::schemas::bricks_components_schema_max();
    let comp = b
        .components
        .iter()
        .find(|c| component_is(c.as_ref(), STRING_CONCAT))
        .expect("String_Concatenate component on the wire's source brick");
    let fid = schema
        .intern
        .get("InputA")
        .unwrap_or_else(|| panic!("InputA interned in max schema"));
    let v = comp
        .as_brdb_struct_prop_value(schema, fid, fid)
        .unwrap_or_else(|e| panic!("InputA present in component data: {e:?}"));
    v.as_brdb_str()
        .unwrap_or_else(|e| panic!("InputA is a string: {e:?}"))
        .to_string()
}

/// `chip P(v: string) -> (r: string)`: the argument is a constant-folded
/// `FormatText` interpolation (`"n=${n}"`, `n = 1000`), known only once
/// `FormatText` folding runs. A bare string literal argument would deliver via
/// the fold-independent string-literal carrier and prove nothing.
const FOLDED_STRING_EXPR: &str = "chip P(v: string) -> (r: string) { out r = v .. Opaque(\"\") }\n\
                                   let n = 1000\n\
                                   out y = P(\"n=${n}\")\n";

#[test]
fn folded_const_string_arg_reaches_named_chip_input() {
    // `v`'s known value is rewired straight to its real consumer
    // (`v .. Opaque("")`), so the pin ends up wire-free. Strings cannot inline
    // as baked wire-variant data (see `operand_source_brick`), so the value
    // arrives via one small carrier gate wired directly into the consumer.
    let w = world(FOLDED_STRING_EXPR, FoldMode::ForceOn);
    let pin = chip_input_brick_id(&w);
    assert_eq!(
        chip_input_incoming_wire_count(&w, pin),
        0,
        "the chip input pin must be wire-free — its value is delivered directly to `v .. \
         Opaque(\"\")` instead"
    );
    let consumer = live_consumer_fed_by_opaque(&w, STRING_CONCAT);
    let carrier = operand_source_brick(&w, consumer, "InputA");
    let text = string_concat_input_a(carrier);
    assert_eq!(
        text, "n=1,000",
        "comma-grouped render law must survive to emit"
    );
    println!(
        "P(\"n=${{n}}\") folded: delivered \"{text}\" directly to the live consumer (pin {pin} \
         wire-free)"
    );
}

/// `chip P(v: vector) -> (r: vector)`: the argument is a folded composite
/// `MathAdd` (`Vec(1,2,3) + Vec(0.5,0.5,0.5)`), known only once `compositeMath`
/// folding runs.
const FOLDED_VECTOR_EXPR: &str = "chip P(v: vector) -> (r: vector) { out r = v + Opaque(Vec(0.0, 0.0, 0.0)) }\n\
     out y = P(Vec(1.0, 2.0, 3.0) + Vec(0.5, 0.5, 0.5))\n";

#[test]
fn folded_const_vector_arg_reaches_named_chip_input() {
    // `v`'s known vector value is rewired straight to its real consumer
    // (`v + Opaque(...)`), so the pin ends up wire-free. A vector inlines as a
    // generic wire-variant operand, so it is baked onto the surviving MathAdd's
    // own `InputA` with no carrier gate.
    let w = world(FOLDED_VECTOR_EXPR, FoldMode::ForceOn);
    let pin = chip_input_brick_id(&w);
    assert_eq!(
        chip_input_incoming_wire_count(&w, pin),
        0,
        "the chip input pin must be wire-free — its value is delivered directly to \
         `v + Opaque(...)` instead"
    );
    let consumer = live_consumer_fed_by_opaque(&w, MATH_ADD);
    let source = operand_source_brick(&w, consumer, "InputA");
    assert_eq!(
        source.id, consumer.id,
        "a Vector must inline as baked data directly on the consumer, not via a separate carrier"
    );
    let (x, y, z) = read_wire_value_vector(source, MATH_ADD, "InputA");
    assert_eq!((x, y, z), (1.5, 2.5, 3.5), "delivered constant vector");
    println!(
        "P(Vec+Vec) folded: delivered ({x}, {y}, {z}) directly to the live consumer (pin {pin} wire-free)"
    );
}

/// `chip P(v)` whose body cannot fully fold (`Opaque` never folds): the folded
/// `2 + 2` argument must still reach the pin as 4.
const FOLDED_EXPR: &str = "chip P(v: int) -> (r: int) { out r = v + Opaque(0) }\n\
                           out y = P(2 + 2)\n";

/// Fold-independent shape: a top-level literal alias passed to a named chip
/// lowers to the same parent-literal to pin wire.
const LET_ALIAS: &str = "chip P(v: int) -> (r: int) { out r = v + Opaque(0) }\n\
                         let k = 4\n\
                         out y = P(k)\n";

/// Like `assert_pin_delivery`, but for the boundary-delivery-cleanup shape
/// (`FoldMode::ForceOn` only; `--no-fold` delivers via the pin): the pin ends
/// up wire-free, and the value is baked onto the real `v + Opaque(0)`
/// consumer's own `InputA` (ints inline as a generic wire-variant operand).
fn assert_direct_delivery(what: &str, src: &str, expected: i64) {
    let w = world(src, FoldMode::ForceOn);
    let pin = chip_input_brick_id(&w);
    assert_eq!(
        chip_input_incoming_wire_count(&w, pin),
        0,
        "{what}: the chip input pin must be wire-free — its value is delivered directly to \
         `v + Opaque(0)` instead"
    );
    let consumer = live_consumer_fed_by_opaque(&w, MATH_ADD);
    let source = operand_source_brick(&w, consumer, "InputA");
    assert_eq!(
        source.id, consumer.id,
        "{what}: an int must inline as baked data directly on the consumer, not via a separate \
         carrier"
    );
    let value = read_wire_value_int(source, MATH_ADD, "InputA");
    assert_eq!(value, expected, "{what}: delivered constant value");
    println!("{what}: delivered {value} directly to the live consumer (pin {pin} wire-free)");
}

#[test]
fn folded_const_expr_arg_reaches_named_chip_input() {
    assert_direct_delivery("P(2 + 2) folded", FOLDED_EXPR, 4);
}

#[test]
fn let_alias_const_arg_reaches_named_chip_input_folded() {
    assert_direct_delivery("let k = 4; P(k) folded", LET_ALIAS, 4);
}

#[test]
fn let_alias_const_arg_reaches_named_chip_input_no_fold() {
    assert_pin_delivery(
        "let k = 4; P(k) --no-fold",
        LET_ALIAS,
        FoldMode::ForceOff,
        4,
    );
}

/// Control: with folding disabled the expression argument stays a real
/// `MathAdd(2, 2)` wired cross-grid into the pin, the delivery shape the folded
/// variants above must still reproduce.
#[test]
fn unfolded_expr_arg_reaches_named_chip_input() {
    assert_pin_delivery("P(2 + 2) --no-fold", FOLDED_EXPR, FoldMode::ForceOff, 4);
}

/// `chip P(v: quat) -> (r: quat)`: the argument is a folded composite
/// `MakeQuaternion` whose four components are each an arithmetic expression
/// (`0.38 + 0.0`, not a bare literal). A bare-literal `Quat(...)` argument bakes
/// its floats into the `MakeQuaternion` gate's own properties during call
/// lowering and never produces a `Literal::Quat` (`expr_to_literal` has no
/// `Quat` arm), so it would not reach the `Literal::Quat` carrier arm under
/// test. Arithmetic arguments force a `MathAdd` per component, so a
/// `Literal::Quat` appears only once the fold pass evaluates `MakeQuaternion`
/// with all four inputs known (`fold/eval.rs::make_quaternion`). `Slerp` is a
/// deferred op (see `DEFERRED` in `fold/eval.rs`) and never folds, so `v` stays
/// a real gate inside the chip body and `P` remains a cross-module boundary.
const FOLDED_QUAT_EXPR: &str = "chip P(v: quat) -> (r: quat) { out r = v.Slerp(Opaque(Quat(0.0, 0.0, 0.0, 1.0)), 0.0) }\n\
     out y = P(Quat(0.0 + 0.0, 0.0 + 0.0, 0.38 + 0.0, 0.92 + 0.0))\n";

const QUAT_SLERP: &str = "BrickComponentType_Expr_QuatSlerp";

#[test]
fn folded_const_quat_arg_reaches_named_chip_input() {
    // `v`'s known quat value is rewired straight to its real consumer
    // (`v.Slerp(Opaque(...), 0.0)`), so the pin ends up wire-free. A quat
    // inlines as a generic wire-variant operand, so it is baked onto the
    // surviving QuatSlerp's own receiver input with no carrier gate.
    let w = world(FOLDED_QUAT_EXPR, FoldMode::ForceOn);
    let pin = chip_input_brick_id(&w);
    assert_eq!(
        chip_input_incoming_wire_count(&w, pin),
        0,
        "the chip input pin must be wire-free — its value is delivered directly to \
         `v.Slerp(...)` instead"
    );
    let consumer = live_consumer_fed_by_opaque(&w, QUAT_SLERP);
    let source = operand_source_brick(&w, consumer, "InputA");
    assert_eq!(
        source.id, consumer.id,
        "a Quat must inline as baked data directly on the consumer, not via a separate carrier"
    );
    let (x, y, z, wq) = read_wire_value_quat(source, QUAT_SLERP, "InputA");
    assert_eq!(
        (x, y, z, wq),
        (0.0, 0.0, 0.38, 0.92),
        "delivered constant quat"
    );
    println!(
        "P(Quat(arith)) folded: delivered ({x}, {y}, {z}, {wq}) directly to the live consumer \
         (pin {pin} wire-free)"
    );
}

const FORMAT_TEXT: &str = "BrickComponentType_Expr_String_FormatText";
const CHIP_OUT: &str = "BrickComponentType_Internal_MicrochipOutput";

/// The wires feeding `port` on brick `brick_id`.
fn incoming<'w>(w: &'w brdb::World, brick_id: usize, port: &str) -> Vec<&'w brdb::WireConnection> {
    w.wires
        .iter()
        .filter(|c| c.target.brick_id == brick_id && c.target.port_name.to_string() == port)
        .collect()
}

/// Follow the wire feeding `port` on `consumer` back through boundary pins and
/// rerouters to the brick that produces the value.
fn traced_source<'w>(w: &'w brdb::World, consumer: &brdb::Brick, port: &str) -> &'w brdb::Brick {
    let (mut brick_id, mut port) = (
        consumer.id.expect("consumer brick has an id"),
        port.to_string(),
    );
    loop {
        let feeds = incoming(w, brick_id, &port);
        assert_eq!(
            feeds.len(),
            1,
            "brick {brick_id} port {port} must have exactly one incoming wire; wires: {:?}",
            w.wires
        );
        let source = find_brick(w, feeds[0].source.brick_id);
        let is_carrier = source.components.iter().any(|c| {
            component_is(c.as_ref(), CHIP_IN)
                || component_is(c.as_ref(), CHIP_OUT)
                || component_is(c.as_ref(), REROUTER)
        });
        if !is_carrier {
            return source;
        }
        brick_id = feeds[0].source.brick_id;
        port = "RER_Input".to_string();
    }
}

/// A constant a consumer reads: numbers compare as `f64`, so a baked `i64` and
/// an `f64` of the same value are equal.
#[derive(Debug, PartialEq)]
enum Const {
    Num(f64),
    Str(String),
}

impl From<WireValue> for Const {
    fn from(v: WireValue) -> Self {
        match v {
            WireValue::Int(n) => Self::Num(n as f64),
            WireValue::Number(f) => Self::Num(f),
            WireValue::Str(s) => Self::Str(s),
            other => panic!("unexpected constant {other:?}"),
        }
    }
}

/// The constant field `field` of the `component_type` component on `b`: a
/// wire value, or a native `str` field.
fn baked_field(b: &brdb::Brick, component_type: &str, field: &str) -> Option<Const> {
    let schema = brdb::schemas::bricks_components_schema_max();
    let comp = b
        .components
        .iter()
        .find(|c| component_is(c.as_ref(), component_type))?;
    let fid = schema.intern.get(field)?;
    let v = comp.as_brdb_struct_prop_value(schema, fid, fid).ok()?;
    match as_wire_value(v) {
        Some(w) => Some(w.into()),
        None => v.as_brdb_str().ok().map(|s| Const::Str(s.to_string())),
    }
}

/// The value a constant gate with no wired input produces: a string carrier or
/// concatenation, or a `MathAdd`/`MathMultiply` of two baked operands (the
/// unfolded form of a constant expression).
fn constant_output(w: &brdb::World, b: &brdb::Brick) -> Const {
    let id = b.id.expect("brick has an id");
    let class = b
        .components
        .iter()
        .filter_map(|c| c.component_type().map(|t| t.to_string()))
        .find(|t| t.starts_with("BrickComponentType_Expr_"))
        .unwrap_or_else(|| panic!("brick {id} is not an expression gate"));
    for port in ["InputA", "InputB"] {
        assert!(
            incoming(w, id, port).is_empty(),
            "brick {id} {class}.{port} is wired, not constant"
        );
    }
    let operand = |field: &str| baked_field(b, &class, field);
    match class.as_str() {
        STRING_CONCAT => {
            let text = |field: &str| match operand(field) {
                Some(Const::Str(s)) => s,
                None => String::new(),
                Some(other) => panic!("{field} holds {other:?}"),
            };
            let (a, sep, rest) = (text("InputA"), text("Separator"), text("InputB"));
            Const::Str(if rest.is_empty() {
                a
            } else {
                format!("{a}{sep}{rest}")
            })
        }
        MATH_ADD | MATH_MULTIPLY => match (operand("InputA"), operand("InputB")) {
            (Some(Const::Num(a)), Some(Const::Num(b))) => {
                Const::Num(if class == MATH_ADD { a + b } else { a * b })
            }
            other => panic!("brick {id} {class} operands {other:?}"),
        },
        _ => panic!("brick {id} {class} is not a constant gate this test evaluates"),
    }
}

/// The constant `consumer` reads on `port`: baked into its own data, or
/// produced by the constant gate its wire traces back to.
fn port_value(w: &brdb::World, consumer: &brdb::Brick, component_type: &str, port: &str) -> Const {
    let id = consumer.id.expect("consumer brick has an id");
    if !incoming(w, id, port).is_empty() {
        return constant_output(w, traced_source(w, consumer, port));
    }
    baked_field(consumer, component_type, port)
        .unwrap_or_else(|| panic!("brick {id} {component_type}.{port} is neither wired nor baked"))
}

const MATH_MULTIPLY: &str = "BrickComponentType_Expr_MathMultiply";

/// Every `class` gate whose `input_port` is wired (the gates reading the
/// runtime input) must read `expected` on `const_port`, with folding on and
/// off. `count` is how many such gates the program has.
fn assert_consumers_read(
    what: &str,
    src: &str,
    class: &str,
    const_port: &str,
    input_port: &str,
    expected: &Const,
    count: usize,
) {
    for fold_mode in [FoldMode::ForceOn, FoldMode::ForceOff] {
        let w = world(src, fold_mode);
        let consumers: Vec<&brdb::Brick> = w
            .bricks
            .iter()
            .chain(w.grids.iter().flat_map(|(_, bricks)| bricks.iter()))
            .filter(|b| b.components.iter().any(|c| component_is(c.as_ref(), class)))
            .filter(|b| !incoming(&w, b.id.expect("brick has an id"), input_port).is_empty())
            .collect();
        assert_eq!(
            consumers.len(),
            count,
            "{what} ({fold_mode:?}): {class} gates reading the input"
        );
        for consumer in consumers {
            assert_eq!(
                port_value(&w, consumer, class, const_port),
                *expected,
                "{what} ({fold_mode:?}): {class}.{const_port} must read the constant"
            );
        }
    }
}

/// A top-level string constant read by the root module and by a named chip:
/// the chip's reads are wires stored in the chip's module whose source lives in
/// the root. Folding `"${CUR}${5}"` away leaves the root with one consumer of
/// `CUR`; literal inlining must not treat it as single-use, bake it into that
/// consumer and delete it, or the chip's wire is stranded (a `dropped wire` emit
/// error). The other two shapes reach the same state without folding: one root
/// consumer to begin with, or a root consumer that bakes the string into a
/// native `str` field before folding runs.
const SHARED_CONST_FOLDED_TO_ONE_USE: &str = "let CUR = \"$\"\n\
     in n: int\n\
     chip show(s: int) -> string {\n  return \"${CUR}${s}\"\n}\n\
     out a = \"${CUR}${n}\"\n\
     out c = \"${CUR}${5}\"\n\
     out b = show(n)\n";

const SHARED_CONST_ONE_ROOT_USE: &str = "let CUR = \"$\"\n\
     in n: int\n\
     chip show(s: int) -> string {\n  return \"${CUR}${s}\"\n}\n\
     out a = \"${CUR}${n}\"\n\
     out b = show(n)\n";

const SHARED_CONST_BAKED_ROOT_USE: &str = "let SEP = \"|\"\n\
     in n: string\n\
     chip show(s: string) -> string {\n  return s .. SEP\n}\n\
     out a = n .. SEP\n\
     out b = show(n)\n";

#[test]
fn shared_string_constant_still_reaches_named_chip() {
    let s = |t: &str| Const::Str(t.to_string());
    assert_consumers_read(
        "folded to one root use",
        SHARED_CONST_FOLDED_TO_ONE_USE,
        FORMAT_TEXT,
        "InputA",
        "InputB",
        &s("$"),
        2,
    );
    assert_consumers_read(
        "one root use",
        SHARED_CONST_ONE_ROOT_USE,
        FORMAT_TEXT,
        "InputA",
        "InputB",
        &s("$"),
        2,
    );
    assert_consumers_read(
        "baked root use",
        SHARED_CONST_BAKED_ROOT_USE,
        STRING_CONCAT,
        "InputB",
        "InputA",
        &s("|"),
        2,
    );
}

/// A top-level constant expression read by a named chip. Folding turns it into
/// a `_Literal` in the root, and a literal reaches only its own module's
/// consumers (emit inlines it; a boundary pin never carries one), so the chip
/// needs its own copy or its gate reads the port default (0 or "").
#[test]
fn folded_constant_expression_reaches_named_chip() {
    let n = Const::Num;
    // Read by the root and the chip.
    let k1 = "let K = 3 + 4\nin n: int\nchip show(s: int) -> int {\n  return s + K\n}\n\
              out a = n + K\nout b = show(n)\n";
    assert_consumers_read("int", k1, MATH_ADD, "InputB", "InputA", &n(7.0), 2);
    // Two root reads, so the root keeps its literal.
    let k2 = "let K = 3 + 4\nin n: int\nin m: int\nchip show(s: int) -> int {\n  return s * K\n}\n\
              out a = n + K\nout c = m - K\nout b = show(n)\n";
    assert_consumers_read(
        "int, two root reads",
        k2,
        MATH_MULTIPLY,
        "InputB",
        "InputA",
        &n(7.0),
        1,
    );
    let k3 = "let CUR = \"a\" .. \"b\"\nin n: int\nchip show(s: int) -> string {\n  return \"${CUR}${s}\"\n}\n\
              out a = \"${CUR}${n}\"\nout b = show(n)\n";
    assert_consumers_read(
        "string",
        k3,
        FORMAT_TEXT,
        "InputA",
        "InputB",
        &Const::Str("ab".into()),
        2,
    );
    // A constant built on another constant.
    let k4 = "let K = 7\nlet J = K * 2\nin n: int\nchip show(s: int) -> int {\n  return s * J\n}\n\
              out a = n + J\nout b = show(n)\n";
    assert_consumers_read(
        "chained",
        k4,
        MATH_MULTIPLY,
        "InputB",
        "InputA",
        &n(14.0),
        1,
    );
    // Read only by the chip.
    let k6 = "let K = 3 + 4\nin n: int\nchip show(s: int) -> int {\n  return s * K\n}\nout b = show(n)\n";
    assert_consumers_read(
        "chip only",
        k6,
        MATH_MULTIPLY,
        "InputB",
        "InputA",
        &n(7.0),
        1,
    );
    let k7 = "const K = 3 + 4\nin n: int\nchip show(s: int) -> int {\n  return s * K\n}\n\
              out a = n + K\nout b = show(n)\n";
    assert_consumers_read("const", k7, MATH_MULTIPLY, "InputB", "InputA", &n(7.0), 1);
    let k9 = "let K = 2.5 * 2.0\nin n: float\nchip show(s: float) -> float {\n  return s * K\n}\n\
              out a = n + K\nout c = n - K\nout b = show(n)\n";
    assert_consumers_read("float", k9, MATH_MULTIPLY, "InputB", "InputA", &n(5.0), 1);
}
