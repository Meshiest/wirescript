//! Certified evaluator: implements exactly the laws the in-game probe
//! certified (data/gate_semantics.json). `None` = refuse to fold. The replay
//! test at the bottom locks every table case to this implementation — table
//! and evaluator cannot drift apart without breaking the build.
use crate::ir::Literal;
use crate::lower::fold::table::InVariant;

// `pub`: reachable as `wirescript::lower::fold::eval::Value` from the
// `--fold-diff` fuzz harness (see the visibility note on `pub mod fold` in
// `lower/mod.rs`). `variant()` stays `pub(crate)` since it returns the
// crate-private `InVariant` — the harness never needs it (it re-derives
// coverage indirectly by calling `eval()`, which already gates on it).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl Value {
    pub(crate) fn variant(&self) -> InVariant {
        match self {
            Value::Int(_) => InVariant::Int,
            Value::Float(_) => InVariant::Float,
            Value::Bool(_) => InVariant::Bool,
            Value::Str(_) => InVariant::Str,
        }
    }
    pub fn from_literal(lit: &Literal) -> Option<Value> {
        match lit {
            Literal::Int(n) => Some(Value::Int(*n)),
            Literal::Float(f) => Some(Value::Float(*f)),
            Literal::Bool(b) => Some(Value::Bool(*b)),
            Literal::String(s) => Some(Value::Str(s.clone())),
            _ => None, // vector/rotator/color/object/array: not certified
        }
    }
    pub fn to_literal(&self) -> Literal {
        match self {
            Value::Int(n) => Literal::Int(*n),
            Value::Float(f) => Literal::Float(*f),
            Value::Bool(b) => Literal::Bool(*b),
            Value::Str(s) => Literal::String(s.clone()),
        }
    }
}

/// Certified: non-finite floats sanitize to 0 at gate INPUTS.
pub(crate) fn sanitize(f: f64) -> f64 {
    if f.is_finite() { f } else { 0.0 }
}

/// Certified truthiness (Select/Branch chapters): int nonzero; sanitized
/// float nonzero; string not in {"", "0", "false"}; bool as-is; unwired falsy.
pub(crate) fn truthy(v: Option<&Value>) -> bool {
    match v {
        None => false,
        Some(Value::Int(n)) => *n != 0,
        Some(Value::Float(f)) => sanitize(*f) != 0.0,
        Some(Value::Bool(b)) => *b,
        Some(Value::Str(s)) => !matches!(s.as_str(), "" | "0" | "false"),
    }
}

/// FormatText-style rendering — used ONLY to compare eval results against the
/// table's recorded (lossy) console output, never to produce fold results.
pub(crate) fn render(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => s.clone(),
        Value::Float(f) => {
            if !f.is_finite() || *f == 0.0 {
                "0".to_string() // covers -0.0 too
            } else {
                format!("{f}") // Rust Display: 1.0 -> "1", 0.75 -> "0.75"
            }
        }
    }
}

/// i64 -> f64 is exact only within ±2^53; beyond that, refuse mixed-domain
/// numeric work (never probed at that magnitude).
fn int_as_f64(n: i64) -> Option<f64> {
    const LIMIT: i64 = 1 << 53;
    (-LIMIT..=LIMIT).contains(&n).then_some(n as f64)
}

/// Numeric coercion for ORDERED compares only (certified parse-or-zero,
/// incl. str: "10" vs "9" compares 10 > 9; "a" -> 0).
fn ord_num(v: Option<&Value>) -> Option<f64> {
    Some(match v {
        None => 0.0,
        Some(Value::Int(n)) => int_as_f64(*n)?,
        Some(Value::Float(f)) => sanitize(*f),
        Some(Value::Bool(b)) => *b as i64 as f64,
        Some(Value::Str(s)) => s.parse::<f64>().map(sanitize).unwrap_or(0.0),
    })
}

/// EQ/NE: certified per variant pair. Canonical-string for int-vs-str; exact
/// for str-str; numeric for int/float/bool mixes. Unprobed pairs return None
/// (the coverage gate blocks them anyway — this is belt and suspenders).
fn eq(a: Option<&Value>, b: Option<&Value>) -> Option<bool> {
    use Value::*;
    // Certified: unwired behaves as the other operand's domain default.
    let default_for = |other: &Value| -> Value {
        match other {
            Int(_) => Int(0),
            Float(_) => Float(0.0),
            Bool(_) => Bool(false),
            Str(_) => Str(String::new()),
        }
    };
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a.clone(), b.clone()),
        (Some(a), None) => { let d = default_for(a); (a.clone(), d) }
        (None, Some(b)) => { let d = default_for(b); (d, b.clone()) }
        (None, None) => return None, // not in table
    };
    Some(match (&a, &b) {
        (Int(x), Int(y)) => x == y,
        (Float(x), Float(y)) => sanitize(*x) == sanitize(*y),
        (Int(x), Float(y)) | (Float(y), Int(x)) => int_as_f64(*x)? == sanitize(*y),
        (Int(x), Bool(y)) | (Bool(y), Int(x)) => *x == *y as i64,
        (Bool(x), Bool(y)) => x == y,
        (Str(x), Str(y)) => x == y,
        // Certified (int:N str:S direction): canonical int rendering compared
        // as a string ("1"=="1" T, ""=="0"'s int-0 F, "1.0" F).
        (Int(x), Str(s)) | (Str(s), Int(x)) => &x.to_string() == s,
        _ => return None, // float-str / bool-str / float-bool: unprobed
    })
}

fn cmp(a: Option<&Value>, b: Option<&Value>) -> Option<std::cmp::Ordering> {
    // Pure-int stays in i64 (64-bit compares are certified past 2^53).
    if let (Some(Value::Int(x)), Some(Value::Int(y))) = (a, b) {
        return Some(x.cmp(y));
    }
    ord_num(a)?.partial_cmp(&ord_num(b)?)
}

/// Math domain: Int/Bool/unwired stay in exact i64 (checked); any Float
/// switches to f64 with input sanitize. Strings refuse (certified outputs
/// contradict every parse model: "a"+1 recorded 0, not 1).
enum MathIn { I(i64), F(f64) }
fn math_in(v: Option<&Value>) -> Option<MathIn> {
    Some(match v {
        None => MathIn::I(0),
        Some(Value::Int(n)) => MathIn::I(*n),
        Some(Value::Bool(b)) => MathIn::I(*b as i64),
        Some(Value::Float(f)) => MathIn::F(sanitize(*f)),
        Some(Value::Str(_)) => return None,
    })
}

fn math(gate: &str, a: Option<&Value>, b: Option<&Value>) -> Option<Value> {
    let (x, y) = (math_in(a)?, math_in(b)?);
    // Mixed int/float promotes to float (certified: 1 + 0.5 = 1.5).
    if let (MathIn::I(x), MathIn::I(y)) = (&x, &y) {
        let (x, y) = (*x, *y);
        return Some(Value::Int(match gate {
            g if g.ends_with("MathAdd") => x.checked_add(y)?,
            g if g.ends_with("MathSubtract") => x.checked_sub(y)?,
            g if g.ends_with("MathMultiply") => x.checked_mul(y)?,
            g if g.ends_with("MathDivide") => {
                if y == 0 { 0 } // certified: div by zero -> 0
                else {
                    // checked_rem first: None on i64::MIN % -1 (the same
                    // overflow that would make raw `x / y` panic below).
                    let rem = x.checked_rem(y)?;
                    // Truncation direction unprobed for mixed signs with a
                    // remainder — refuse rather than guess trunc vs floor.
                    if (x < 0) != (y < 0) && rem != 0 { return None; }
                    x.checked_div(y)?
                }
            }
            g if g.ends_with("MathModulo") => {
                if y == 0 { 0 } // certified: mod by zero -> 0
                else {
                    // checked_rem first: None on i64::MIN % -1 refuses before
                    // any raw `%` can panic (the guard itself must not panic).
                    let rem = x.checked_rem(y)?;
                    if (x < 0 || y < 0) && rem != 0 { return None; }
                    rem
                }
            }
            _ => return None,
        }));
    }
    let to_f = |m: MathIn| -> Option<f64> {
        Some(match m { MathIn::I(n) => int_as_f64(n)?, MathIn::F(f) => f })
    };
    let (x, y) = (to_f(x)?, to_f(y)?);
    if gate.ends_with("MathModulo") {
        let r = x % y;
        // Mirrors the int-path mixed-sign refusal: truncation direction is
        // unprobed for mixed signs with a nonzero (finite) remainder.
        if (x < 0.0) != (y < 0.0) && r != 0.0 && r.is_finite() {
            return None;
        }
        return Some(Value::Float(r));
    }
    Some(Value::Float(match gate {
        g if g.ends_with("MathAdd") => x + y,
        g if g.ends_with("MathSubtract") => x - y,
        g if g.ends_with("MathMultiply") => x * y,
        g if g.ends_with("MathDivide") => x / y,   // non-finite result: fold
        _ => return None,                          // renders as "0"
    }))
}

// `pub`: the `--fold-diff` fuzz harness's predictor calls this directly
// instead of re-implementing the certified value laws — see the visibility
// note on `pub mod fold` in `lower/mod.rs`.
pub fn eval(gate_class: &str, inputs: &[Option<Value>]) -> Option<Value> {
    // Coverage gate: refuse any input-shape the in-game probe never actually
    // observed for this gate class (unwired operands on unprobed gates, the
    // un-probed direction of an asymmetric compare, Bool in ordered compares,
    // NOT on non-bool, ...). This makes eval exactly as permissive as the
    // certified table, no more — every law below only ever sees a signature
    // that was actually certified. (A Task-3 driver-side coverage check is a
    // deliberately redundant second layer — belt and suspenders by design.)
    let sig: Vec<InVariant> = inputs
        .iter()
        .map(|v| v.as_ref().map_or(InVariant::Unwired, Value::variant))
        .collect();
    if !crate::lower::fold::table::CertifiedTable::certified().covers(gate_class, &sig) {
        return None;
    }

    let one = |i: usize| inputs.get(i).and_then(|v| v.as_ref());
    let (a, b) = (one(0), one(1));
    let short = gate_class.rsplit('_').next().unwrap_or("");
    Some(match short {
        "CompareEqual" => Value::Bool(eq(a, b)?),
        "CompareNotEqual" => Value::Bool(!eq(a, b)?),
        "CompareLess" => Value::Bool(cmp(a, b)? == std::cmp::Ordering::Less),
        "CompareLessOrEqual" => Value::Bool(cmp(a, b)? != std::cmp::Ordering::Greater),
        "CompareGreater" => Value::Bool(cmp(a, b)? == std::cmp::Ordering::Greater),
        "CompareGreaterOrEqual" => Value::Bool(cmp(a, b)? != std::cmp::Ordering::Less),
        "LogicalAND" => Value::Bool(truthy(a) && truthy(b)),
        "LogicalOR" => Value::Bool(truthy(a) || truthy(b)),
        "LogicalXOR" => Value::Bool(truthy(a) != truthy(b)),
        "LogicalNOT" => Value::Bool(!truthy(a)),
        s if s.starts_with("Math") => math(gate_class, a, b)?,
        _ => return None, // Select/Branch handled by the driver; rest unknown
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::fold::table::{CertifiedTable, InVariant};

    const SELECT: &str = "BrickComponentType_WireGraph_Expr_Select";
    const BRANCH: &str = "BrickComponentType_WireGraph_Exec_Branch";

    fn case_value(ci: &crate::lower::fold::table::CaseInput) -> Option<Value> {
        let v = ci.value.as_ref()?;
        Some(match ci.variant {
            InVariant::Int => Value::Int(v.parse().expect("int case value")),
            InVariant::Float => Value::Float(match v.as_str() {
                "NaN" => f64::NAN,
                "inf" => f64::INFINITY,
                "-inf" => f64::NEG_INFINITY,
                _ => v.parse().expect("float case value"),
            }),
            InVariant::Bool => Value::Bool(v == "true"),
            InVariant::Str => Value::Str(v.clone()),
            InVariant::Unwired => unreachable!(),
        })
    }

    /// Signatures eval deliberately refuses (Global Constraints). Every table
    /// case must either replay exactly or match one of these; the count is
    /// asserted so laws can't silently rot into refusals.
    fn is_expected_refusal(gate: &str, sig: &[InVariant]) -> bool {
        gate.contains("_Math") && sig.contains(&InVariant::Str)
    }

    #[test]
    fn replay_every_certified_case() {
        let t = CertifiedTable::certified();
        let (mut replayed, mut refused) = (0usize, 0usize);
        for gate in t.gate_classes() {
            for case in t.cases(gate) {
                let inputs: Vec<Option<Value>> =
                    case.inputs.iter().map(case_value).collect();
                if gate == SELECT || gate == BRANCH {
                    // Truthiness gates: recorded output encodes which side won.
                    let want_truthy = match case.output_value.as_str() {
                        "111" | "A" => true,
                        "222" | "B" => false,
                        other => panic!("{gate}: unexpected output {other:?}"),
                    };
                    assert_eq!(truthy(inputs[0].as_ref()), want_truthy,
                        "{gate} {:?}", case.inputs);
                    replayed += 1;
                    continue;
                }
                let sig: Vec<InVariant> =
                    case.inputs.iter().map(|i| i.variant).collect();
                match eval(gate, &inputs) {
                    Some(v) => {
                        assert_eq!(render(&v), case.output_value,
                            "{gate} {:?} evaluated {v:?}", case.inputs);
                        replayed += 1;
                    }
                    None => {
                        assert!(is_expected_refusal(gate, &sig),
                            "{gate} {:?}: unexpected refusal", case.inputs);
                        // Every refused observation recorded 0 — if a future
                        // probe contradicts that, this screams.
                        assert_eq!(case.output_value, "0");
                        refused += 1;
                    }
                }
            }
        }
        assert_eq!(replayed + refused, 221, "table case count changed — re-audit");
        assert_eq!(refused, 3, "math-with-string refusals (Add: a+b, ''+x, a+1)");
    }

    #[test]
    fn refusal_overflow_and_mixed_sign_div() {
        let add = "BrickComponentType_WireGraph_Expr_MathAdd";
        let div = "BrickComponentType_WireGraph_Expr_MathDivide";
        let md = "BrickComponentType_WireGraph_Expr_MathModulo";
        let i = |n: i64| Some(Value::Int(n));
        assert!(eval(add, &[i(i64::MAX), i(1)]).is_none(), "overflow refuses");
        assert!(eval(div, &[i(-7), i(2)]).is_none(), "trunc-vs-floor unprobed");
        assert!(eval(div, &[i(-4), i(2)]).is_some(), "zero remainder is safe");
        assert!(eval(md, &[i(-7), i(2)]).is_none());
        assert_eq!(eval(div, &[i(7), i(0)]), Some(Value::Int(0)), "div0 certified");
    }

    #[test]
    fn overflow_min_div_neg_one_refuses_no_panic() {
        let div = "BrickComponentType_WireGraph_Expr_MathDivide";
        let md = "BrickComponentType_WireGraph_Expr_MathModulo";
        let i = |n: i64| Some(Value::Int(n));
        // i64::MIN / -1 (and MIN % -1) overflow i64 unconditionally in Rust;
        // must refuse via checked ops rather than panic.
        assert!(eval(div, &[i(i64::MIN), i(-1)]).is_none());
        assert!(eval(md, &[i(i64::MIN), i(-1)]).is_none());
    }

    #[test]
    fn float_modulo_mixed_sign_refuses() {
        let md = "BrickComponentType_WireGraph_Expr_MathModulo";
        let f = |n: f64| Some(Value::Float(n));
        assert!(eval(md, &[f(-3.5), f(2.0)]).is_none());
    }

    #[test]
    fn uncovered_signatures_refuse() {
        let ne = "BrickComponentType_WireGraph_Expr_CompareNotEqual";
        let eq_gate = "BrickComponentType_WireGraph_Expr_CompareEqual";
        let lt = "BrickComponentType_WireGraph_Expr_CompareLess";
        let not = "BrickComponentType_WireGraph_Expr_LogicalNOT";
        // (Str, Unwired): CompareNotEqual was only probed at (int,int)/(int,str).
        assert!(eval(ne, &[Some(Value::Str("x".into())), None]).is_none());
        // Reverse of the probed (int,str) direction — (str,int) unprobed.
        assert!(eval(eq_gate,
            &[Some(Value::Str("1".into())), Some(Value::Int(1))]).is_none());
        // Bool never appears as the second operand of an ordered compare.
        assert!(eval(lt, &[Some(Value::Int(1)), Some(Value::Bool(true))]).is_none());
        // NOT was only ever probed on Bool.
        assert!(eval(not, &[Some(Value::Int(1))]).is_none());
    }

    #[test]
    fn covered_signature_still_folds() {
        let eq_gate = "BrickComponentType_WireGraph_Expr_CompareEqual";
        assert_eq!(
            eval(eq_gate, &[Some(Value::Int(1)), Some(Value::Str("1".into()))]),
            Some(Value::Bool(true))
        );
    }

    #[test]
    fn render_matches_formattext() {
        assert_eq!(render(&Value::Float(1.0)), "1");
        assert_eq!(render(&Value::Float(0.75)), "0.75");
        assert_eq!(render(&Value::Float(f64::INFINITY)), "0");
        assert_eq!(render(&Value::Float(f64::NAN)), "0");
        assert_eq!(render(&Value::Float(-0.0)), "0");
        assert_eq!(render(&Value::Bool(true)), "true");
        assert_eq!(render(&Value::Int(-5)), "-5");
    }
}
