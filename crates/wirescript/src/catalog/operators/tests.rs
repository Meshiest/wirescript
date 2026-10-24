use super::*;

const ADD: &str = "BrickComponentType_Expr_MathAdd";
const SUB: &str = "BrickComponentType_Expr_MathSubtract";
const MUL: &str = "BrickComponentType_Expr_MathMultiply";
const DIV: &str = "BrickComponentType_Expr_MathDivide";
const MOD: &str = "BrickComponentType_Expr_MathModulo";

/// What `resolve_op` must give for a row: no rule, or a rule whose result
/// type and gate class match where given.
#[derive(Debug)]
enum Want {
    NoRule,
    Rule(Option<Type>, Option<&'static str>),
}

fn rule(result: Type, gate: &'static str) -> Want {
    Want::Rule(Some(result), Some(gate))
}

fn typed(result: Type) -> Want {
    Want::Rule(Some(result), None)
}

fn any_rule() -> Want {
    Want::Rule(None, None)
}

#[test]
fn operator_rules() {
    use Type::*;
    let mut rows: Vec<(&str, Vec<Type>, Want)> = vec![
        ("+", vec![Int, Int], rule(Int, ADD)),
        ("+", vec![Int, Float], typed(Float)),
        // Every operand coerces to bool except a vector.
        ("&&", vec![Bool, Bool], any_rule()),
        ("&&", vec![Int, Bool], any_rule()),
        ("&&", vec![Bool, Int], any_rule()),
        ("&&", vec![Exec, Bool], any_rule()),
        ("&&", vec![Entity, Bool], any_rule()),
        ("&&", vec![Controller, Entity], any_rule()),
        ("&&", vec![Float, Exec], any_rule()),
        ("&&", vec![Vector, Bool], Want::NoRule),
        ("-u", vec![Int], any_rule()),
        ("-u", vec![Float], any_rule()),
        ("-u", vec![Bool], Want::NoRule),
        // The AST carries unary negate as "-" (same spelling as binary
        // subtract); resolve_op maps it to the table's "-u" key by arity, so
        // every caller resolves it (an un-remapped `-x` in a generic mod
        // otherwise fell through to a stale fallback type).
        (
            "-",
            vec![Int],
            rule(Int, "BrickComponentType_Expr_MathNegate"),
        ),
        ("-", vec![Float], typed(Float)),
        // Binary "-" (arity 2) stays subtract; the remap is arity-gated.
        ("-", vec![Int, Int], Want::Rule(None, Some(SUB))),
        // Bitwise operators coerce bool and float operands to int.
        ("<<", vec![Bool, Int], typed(Int)),
        ("&", vec![Bool, Bool], any_rule()),
        ("|", vec![Int, Bool], any_rule()),
        ("^", vec![Bool, Int], any_rule()),
        ("<<", vec![Float, Int], typed(Int)),
        ("&", vec![Float, Float], any_rule()),
        ("~", vec![Float], any_rule()),
        ("~", vec![Bool], any_rule()),
    ];
    // bool coerces to 0/1 on int ports, so bool op bool is int-valued, same
    // as bool op int and the bitwise ops. Covers `(a && b) + (c && d)`.
    for op in ["+", "-", "*", "/", "%"] {
        rows.push((op, vec![Bool, Bool], typed(Int)));
    }
    for t in [Bool, Float, Vector, Entity, Controller, Character] {
        rows.push(("..", vec![String, t.clone()], typed(String)));
        rows.push(("..", vec![t, String], typed(String)));
    }
    // vec op vec, vec op scalar (both directions), and the same for color
    // (RGBA channel-wise), all lower to the same math gate and keep their type.
    for (op, gate) in [("+", ADD), ("-", SUB), ("*", MUL), ("/", DIV), ("%", MOD)] {
        for v in [Vector, Color] {
            for operands in [
                [v.clone(), v.clone()],
                [v.clone(), Float],
                [v.clone(), Int],
                [Float, v.clone()],
                [Int, v.clone()],
            ] {
                rows.push((op, operands.to_vec(), rule(v.clone(), gate)));
            }
        }
    }
    // quat / rotator operands ride the same math gates (`q1 * q2` composes two
    // rotations). Same-type keeps its type; a mix yields a quat.
    for (op, gate) in [("+", ADD), ("*", MUL)] {
        rows.push((op, vec![Quat, Quat], rule(Quat, gate)));
        rows.push((op, vec![Rotator, Rotator], typed(Rotator)));
        rows.push((op, vec![Rotator, Quat], typed(Quat)));
    }

    let mut failures = Vec::new();
    for (op, operands, want) in &rows {
        let got = resolve_op(op, operands);
        let ok = match (want, got) {
            (Want::NoRule, None) => true,
            (Want::Rule(result, gate), Some(r)) => {
                result.as_ref().is_none_or(|t| r.result == *t)
                    && gate.is_none_or(|g| r.gate_class == g)
            }
            _ => false,
        };
        if !ok {
            let got = got.map(|r| (&r.result, r.gate_class));
            failures.push(format!("{op} {operands:?}: want {want:?}, got {got:?}"));
        }
    }
    crate::test_support::report_failures(&failures, rows.len());
}
