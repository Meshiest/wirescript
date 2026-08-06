//! Generate a `.ws` circuit that instantiates every builtin call in the
//! catalog, so compiling + pasting the result into Brickadia surfaces gate
//! encoding / load bugs (fixed-size arrays, composite structs, asset refs, …).
//!
//! Usage:
//!   cargo run -p wirescript --example gen_every_builtin -- <out.ws>
//!
//! Each pure call becomes a top-level `let`; each exec call a statement in an
//! `on start` handler. Only REQUIRED params are supplied (dummy values); config
//! is exercised separately by projects/gate-config-samples. Builtins whose
//! required params need a type the generator can't synthesize (exec / prefab /
//! record / map / union / tuple) are skipped and listed in a trailing comment.

use std::collections::BTreeSet;
use wirescript::Type;

/// A dummy value expression of `ty`, for a call argument. `None` if the type
/// can't be synthesized (the caller then skips the whole builtin).
fn arg_expr(ty: &Type, arrays: &mut BTreeSet<&'static str>, refs: &mut BTreeSet<&'static str>) -> Option<String> {
    Some(match ty {
        Type::Bool => "false".into(),
        Type::Int => "0".into(),
        Type::Float => "0.0".into(),
        Type::String => "\"x\"".into(),
        Type::Vector => "Vec(0.0, 0.0, 0.0)".into(),
        Type::Rotator => "Rotation(0.0, 0.0, 0.0)".into(),
        Type::Quat => "Quat(0.0, 0.0, 0.0, 1.0)".into(),
        Type::Color => "Color(1.0, 1.0, 1.0)".into(),
        Type::Entity => "src_entity".into(),
        Type::Character => "src_char".into(),
        Type::Controller => "src_ctrl".into(),
        Type::Any => "0".into(),
        Type::Opaque => "Opaque(0)".into(),
        Type::Array(inner) => {
            let d = value_decl(inner)?;
            arrays.insert(d);
            format!("arr_{d}")
        }
        Type::Ref(inner) => {
            let d = value_decl(inner)?;
            refs.insert(d);
            format!("&ref_{d}")
        }
        // A math-variant union (Float | Int | Vector | …): the first member that
        // can be synthesized works everywhere the union is accepted.
        Type::Union(members) => {
            for m in members {
                if let Some(e) = arg_expr(m, arrays, refs) {
                    return Some(e);
                }
            }
            return None;
        }
        _ => return None,
    })
}

/// The wirescript type keyword for a value type (array element / ref pointee /
/// var declaration). `None` for types that can't be declared as a plain var.
fn value_decl(ty: &Type) -> Option<&'static str> {
    Some(match ty {
        Type::Bool => "bool",
        Type::Int => "int",
        Type::Float => "float",
        Type::String => "string",
        Type::Vector => "vector",
        Type::Rotator => "rotator",
        Type::Quat => "quat",
        Type::Color => "color",
        Type::Entity => "entity",
        Type::Character => "character",
        Type::Controller => "controller",
        _ => return None,
    })
}

/// A plausible external asset ref for a non-wire asset config param (which must
/// be a constant `$Type/Name`, not a variable). Namespace picked by param name.
fn asset_ref_for(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    if n.contains("audio") {
        "$BrickOneShotAudioDescriptor/BOSA_Bow_Fire".into()
    } else if n.contains("font") {
        "$BrickFontDescriptor/Roboto".into()
    } else {
        // weapon / item / entityType / projectile / brick asset
        "$BRItemBase/Weapon_Bow".into()
    }
}

/// An initializer literal for a `var ref_<d>: <d>` binding.
fn value_init(d: &str) -> &'static str {
    match d {
        "int" => "0",
        "float" => "0.0",
        "bool" => "false",
        "string" => "\"\"",
        "vector" => "Vec(0.0, 0.0, 0.0)",
        "color" => "Color(1.0, 1.0, 1.0)",
        "quat" => "Quat(0.0, 0.0, 0.0, 1.0)",
        "rotator" => "Rotation(0.0, 0.0, 0.0)",
        _ => "0",
    }
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: gen_every_builtin <out.ws>");

    let calls = wirescript::catalog::calls::calls();
    let mut specs: Vec<_> = calls.values().collect();
    specs.sort_by_key(|s| s.name);

    let mut arrays: BTreeSet<&'static str> = BTreeSet::new();
    let mut refs: BTreeSet<&'static str> = BTreeSet::new();
    let mut pure_lets: Vec<String> = Vec::new();
    let mut exec_stmts: Vec<String> = Vec::new();
    let mut skipped: Vec<(&str, String)> = Vec::new();

    for spec in &specs {
        let mut args: Vec<String> = Vec::new();
        let mut skip: Option<String> = None;
        for p in &spec.params {
            if p.optional {
                continue;
            }
            // A required non-wire config param that is asset-ref (entity) typed
            // must be a constant `$Type/Name`, not a variable.
            let is_config = !wirescript::catalog::is_wire_input(spec.gate_class, p.port.as_str());
            let arg = if is_config && matches!(p.ty, Type::Entity) {
                Some(asset_ref_for(p.name))
            } else {
                arg_expr(&p.ty, &mut arrays, &mut refs)
            };
            match arg {
                Some(a) => args.push(a),
                None => {
                    skip = Some(format!("param '{}': {:?}", p.name, p.ty));
                    break;
                }
            }
        }
        if let Some(reason) = skip {
            skipped.push((spec.name, reason));
            continue;
        }
        let call = format!("{}({})", spec.name, args.join(", "));
        if spec.exec {
            exec_stmts.push(format!("  {call}"));
        } else {
            pure_lets.push(format!("let out_{} = {call}", spec.name));
        }
    }

    let mut s = String::new();
    s.push_str("// AUTO-GENERATED by examples/gen_every_builtin.rs — do not edit by hand.\n");
    s.push_str("// Instantiates every builtin call so pasting the compiled circuit into\n");
    s.push_str("// Brickadia surfaces gate encoding / load bugs. `on start` is an unwired\n");
    s.push_str("// input exec (never fires); the gates are placed regardless.\n\n");
    s.push_str("in start: exec\n");
    s.push_str("in src_entity: entity\n");
    s.push_str("in src_char: character\n");
    s.push_str("in src_ctrl: controller\n");
    for d in &arrays {
        s.push_str(&format!("var arr_{d}: {d}[]\n"));
    }
    for d in &refs {
        s.push_str(&format!("var ref_{d}: {d} = {}\n", value_init(d)));
    }
    s.push('\n');
    for l in &pure_lets {
        s.push_str(l);
        s.push('\n');
    }
    s.push_str("\non start {\n");
    for st in &exec_stmts {
        s.push_str(st);
        s.push('\n');
    }
    s.push_str("}\n");
    if !skipped.is_empty() {
        s.push_str(&format!(
            "\n// Skipped {} builtin(s) needing a param type the generator can't synthesize:\n",
            skipped.len()
        ));
        for (n, r) in &skipped {
            s.push_str(&format!("//   {n} — {r}\n"));
        }
    }

    std::fs::write(&out, &s).expect("write output");
    eprintln!(
        "wrote {out}: {} pure lets, {} exec stmts, {} skipped",
        pure_lets.len(),
        exec_stmts.len(),
        skipped.len()
    );
}
