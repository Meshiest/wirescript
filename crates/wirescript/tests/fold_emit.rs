//! Emit-level regression tests for constant delivery into NAMED chip inputs.
//!
//! A parent-side `_Literal` wired to a named chip instance's `MicrochipInput`
//! has NO emit-time delivery mechanism: literal-source wires are skipped at
//! emit (they normally inline into the target's data), but a `MicrochipInput`
//! is a rerouter with no matching data field, and `partition_anon_chips`'
//! literal-clone arm only covers nodes moved into ANON chips. The wire was
//! silently dropped and the chip input read 0 — for BOTH the folded
//! `P(2 + 2)` shape and the pre-existing `let k = 4; P(k)` alias shape.
//!
//! These tests compile all the way to an emitted `brdb::World` (the IR-level
//! suite structurally missed this class of bug) and assert the pin actually
//! receives its value: exactly one incoming wire, from a real gate whose
//! embedded data evaluates to the expected constant.

use brdb::schema::WireVariant;
use wirescript::emit::EmitOptions;
use wirescript::{CompileInput, FoldMode, compile_to_world};

const CHIP_IN: &str = "BrickComponentType_Internal_MicrochipInput";
const MATH_ADD: &str = "BrickComponentType_WireGraph_Expr_MathAdd";

fn world(src: &str, fold_mode: FoldMode) -> brdb::World {
    let r = compile_to_world(
        CompileInput {
            source: src,
            file: "fold_emit.ws",
            module_name: None,
            fold_mode,
        },
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

fn component_is(c: &Box<dyn brdb::BrdbComponent>, ty: &str) -> bool {
    c.component_type()
        .map(|t| t.to_string() == ty)
        .unwrap_or(false)
}

/// The chip child grid's `MicrochipInput` brick id (the repro has exactly
/// one value input pin, `v`).
fn chip_input_brick_id(w: &brdb::World) -> usize {
    let mut ids: Vec<usize> = Vec::new();
    for (_entity, bricks) in &w.grids {
        for b in bricks {
            if b.components.iter().any(|c| component_is(c, CHIP_IN)) {
                ids.push(b.id.expect("pin brick has an id"));
            }
        }
    }
    assert_eq!(ids.len(), 1, "expected exactly one MicrochipInput pin brick");
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
/// (`InputA + InputB`) — the value the gate outputs for unwired inputs.
fn math_add_input_sum(b: &brdb::Brick) -> i64 {
    let schema = brdb::schemas::bricks_components_schema_max();
    let comp = b
        .components
        .iter()
        .find(|c| component_is(c, MATH_ADD))
        .expect("MathAdd component on the wire's source brick");
    let read = |field: &str| -> i64 {
        let fid = schema
            .intern
            .get(field)
            .unwrap_or_else(|| panic!("{field} interned in max schema"));
        let v = comp
            .as_brdb_struct_prop_value(schema, fid, fid)
            .unwrap_or_else(|e| panic!("{field} present in component data: {e:?}"));
        match v.as_brdb_wire_variant().expect("wire variant operand") {
            WireVariant::Int(n) => n,
            WireVariant::Number(f) => f as i64,
            other => panic!("unexpected {field} variant: {other:?}"),
        }
    };
    read("InputA") + read("InputB")
}

/// The core assertion: the chip's `v` pin is DELIVERED the constant — one
/// incoming wire, from a real (non-literal) gate that evaluates to `expected`.
fn assert_pin_delivery(what: &str, src: &str, fold_mode: FoldMode, expected: i64) {
    let w = world(src, fold_mode);
    let pin = chip_input_brick_id(&w);
    let incoming: Vec<&brdb::WireConnection> = w
        .wires
        .iter()
        .filter(|c| {
            c.target.brick_id == pin && c.target.port_name.to_string() == "RER_Input"
        })
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
        "{what}: delivered — brick {} {}({sum}) → brick {pin} {CHIP_IN}.RER_Input",
        source.brick_id, source.component_type,
    );
}

/// `chip P(v)` whose body can't fully fold (`Opaque` is certified-opaque):
/// the folded `2 + 2` argument must still reach the pin as 4.
const FOLDED_EXPR: &str = "chip P(v: int) -> (r: int) { out r = v + Opaque(0) }\n\
                           out y = P(2 + 2)\n";

/// Pre-existing (fold-independent) shape: a top-level literal alias passed
/// to a named chip lowers to the same parent-literal → pin wire.
const LET_ALIAS: &str = "chip P(v: int) -> (r: int) { out r = v + Opaque(0) }\n\
                         let k = 4\n\
                         out y = P(k)\n";

#[test]
fn folded_const_expr_arg_reaches_named_chip_input() {
    assert_pin_delivery("P(2 + 2) folded", FOLDED_EXPR, FoldMode::ForceOn, 4);
}

#[test]
fn let_alias_const_arg_reaches_named_chip_input_folded() {
    assert_pin_delivery("let k = 4; P(k) folded", LET_ALIAS, FoldMode::ForceOn, 4);
}

#[test]
fn let_alias_const_arg_reaches_named_chip_input_no_fold() {
    assert_pin_delivery("let k = 4; P(k) --no-fold", LET_ALIAS, FoldMode::ForceOff, 4);
}

/// Control: with folding disabled the expression argument stays a real
/// `MathAdd(2, 2)` wired cross-grid into the pin — this always worked and
/// pins the canonical delivery shape the fix must reproduce.
#[test]
fn unfolded_expr_arg_reaches_named_chip_input() {
    assert_pin_delivery("P(2 + 2) --no-fold", FOLDED_EXPR, FoldMode::ForceOff, 4);
}
