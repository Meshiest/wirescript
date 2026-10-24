//! Generate a `.ws` circuit that places every gate the language can reach:
//! every builtin call, array and map method and event, plus the gates only a
//! language construct makes (storage, loops, `fn`, chips, properties, `as`).
//! Pasting the compiled circuit into Brickadia surfaces gate encoding and load
//! bugs: a gate that fails to load is reported by the game at paste.
//!
//! Usage:
//!   cargo run -p wirescript --example gen_every_builtin -- <out.ws>
//!
//! Writes `<out.ws>`, the compiled `<out.brz>`, and `<out dir>/COVERAGE.md`:
//! which inventory gate classes the save holds, which it does not, and why
//! each builtin left out was left out.
//!
//! Each piece of generated source is a unit. The generator compiles the file
//! and drops any unit a diagnostic lands in (trying its fallback form first),
//! recording why, until the file compiles. Only REQUIRED params are supplied;
//! config is exercised separately by projects/gate-config-samples. The
//! handlers fire only from an unwired `start` input, so nothing runs: this
//! checks that the gates load, not what they compute.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use wirescript::Type;

/// Where a unit goes: the top level, or inside `on start`.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    Top,
    Start,
}

/// One piece of generated source, with fallback forms tried in order.
struct Unit {
    origin: String,
    place: Place,
    forms: Vec<String>,
    /// Index into `forms` of the form in use.
    at: usize,
}

impl Unit {
    fn new(origin: impl Into<String>, place: Place, forms: Vec<String>) -> Self {
        Unit {
            origin: origin.into(),
            place,
            forms,
            at: 0,
        }
    }
}

/// Containers and refs the generated arguments name, declared up front.
#[derive(Default)]
struct Decls {
    arrays: BTreeSet<&'static str>,
    refs: BTreeSet<&'static str>,
}

/// The wirescript type keyword for a value type (array element / ref pointee).
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
        Type::Brick => "brick",
        _ => return None,
    })
}

/// An initializer for a `var ref_<d>: <d>` binding.
fn value_init(d: &str) -> &'static str {
    match d {
        "int" => " = 0",
        "float" => " = 0.0",
        "bool" => " = false",
        "string" => " = \"\"",
        "vector" => " = Vec(0.0, 0.0, 0.0)",
        "color" => " = Color(1.0, 1.0, 1.0)",
        "quat" => " = Quat(0.0, 0.0, 0.0, 1.0)",
        "rotator" => " = Rotation(0.0, 0.0, 0.0)",
        _ => "",
    }
}

/// A dummy value expression of `ty`, for a call argument. `exec` is whether the
/// call sits on an exec chain, where a component can be looked up. `None` if
/// the type can't be synthesized.
fn arg_expr(ty: &Type, exec: bool, d: &mut Decls) -> Option<String> {
    Some(match ty {
        Type::Bool => "false".into(),
        Type::Int | Type::Any | Type::Param(_) => "0".into(),
        Type::Float => "0.0".into(),
        Type::String => "\"x\"".into(),
        Type::Vector => "Vec(0.0, 0.0, 0.0)".into(),
        Type::Rotator => "Rotation(0.0, 0.0, 0.0)".into(),
        Type::Quat => "Quat(0.0, 0.0, 0.0, 1.0)".into(),
        Type::Color => "Color(1.0, 1.0, 1.0)".into(),
        Type::Entity => "src_entity".into(),
        Type::Character => "src_char".into(),
        Type::Controller => "src_ctrl".into(),
        Type::Brick => "GetMicrochipBrick()".into(),
        Type::Zone => "src_zone".into(),
        Type::Teleport => "src_tp".into(),
        Type::Opaque => "Opaque(0)".into(),
        Type::Component(_) if exec => {
            "GetMicrochipBrick().GetComponent($Component/PointLight)".into()
        }
        Type::Map(k, v) if **k == Type::Int && **v == Type::Int => "map_int".into(),
        Type::Array(inner) => {
            let decl = value_decl(inner)?;
            d.arrays.insert(decl);
            format!("arr_{decl}")
        }
        Type::Ref(inner) => match inner.as_ref() {
            Type::Array(e) => {
                let decl = value_decl(e)?;
                d.arrays.insert(decl);
                format!("arr_{decl}")
            }
            Type::Map(k, v) if **k == Type::Int && **v == Type::Int => "map_int".into(),
            other => {
                let decl = value_decl(other)?;
                d.refs.insert(decl);
                format!("&ref_{decl}")
            }
        },
        // A math-variant union (Float | Int | Vector | …): the first member that
        // can be synthesized works everywhere the union is accepted.
        Type::Union(members) => return members.iter().find_map(|m| arg_expr(m, exec, d)),
        _ => return None,
    })
}

/// A plausible external asset ref for a non-wire asset config param (which must
/// be a constant `$Type/Name`, not a variable). Namespace picked by param name.
fn asset_ref_for(name: &str) -> String {
    let n = name.to_ascii_lowercase();
    if n.contains("audio") {
        "$BrickOneShotAudioDescriptor/OSA_Bow_Fire".into()
    } else if n.contains("font") {
        "$BrickFontDescriptor/Roboto".into()
    } else {
        "$BRItemBase/Weapon_Bow".into()
    }
}

/// Builtins whose required arguments the generic synthesis cannot spell: each
/// one's whole call (or, `None`, a reason to leave it to a construct below).
fn call_override(name: &str) -> Option<Result<&'static str, &'static str>> {
    Some(match name {
        "ContainerChange" => Ok("ContainerChange(arr_int)"),
        "GetBrickProperty" => Ok("GetMicrochipBrick().GetBrickProperty(Color)"),
        "SetBrickProperty" => {
            Ok("GetMicrochipBrick().SetBrickProperty(Color, Color(1.0, 1.0, 1.0))")
        }
        "AddComponent" => Ok("GetMicrochipBrick().AddComponent($Component/PointLight)"),
        "GetComponent" => Ok("GetMicrochipBrick().GetComponent($Component/PointLight)"),
        "RemoveComponent" => Ok("GetMicrochipBrick().RemoveComponent($Component/PointLight)"),
        "Branch" | "Union" => Err("an exec construct: `if` makes it (below)"),
        "EnumToInt" | "IntToEnum" => Err("the enum construct below makes it from a runtime value"),
        _ => return None,
    })
}

fn builtin_units(d: &mut Decls, units: &mut Vec<Unit>, skipped: &mut Vec<(String, String)>) {
    let calls = wirescript::catalog::calls::calls();
    let mut specs: Vec<_> = calls.values().collect();
    specs.sort_by_key(|s| s.name);
    for spec in specs {
        let call = match call_override(spec.name) {
            Some(Ok(call)) => call.to_string(),
            Some(Err(why)) => {
                skipped.push((spec.name.to_string(), why.to_string()));
                continue;
            }
            None => {
                let mut args = Vec::new();
                let mut skip = None;
                for p in spec.params.iter().filter(|p| !p.optional) {
                    let is_config =
                        !wirescript::catalog::is_wire_input(spec.gate_class, p.port.as_str());
                    let arg = if is_config && matches!(p.ty, Type::Entity) {
                        Some(asset_ref_for(p.name))
                    } else {
                        arg_expr(&p.ty, spec.exec, d)
                    };
                    match arg {
                        Some(a) => args.push(a),
                        None => {
                            skip = Some(format!(
                                "no dummy value for param `{}` ({:?})",
                                p.name, p.ty
                            ));
                            break;
                        }
                    }
                }
                if let Some(why) = skip {
                    skipped.push((spec.name.to_string(), why));
                    continue;
                }
                format!("{}({})", spec.name, args.join(", "))
            }
        };
        let origin = format!("builtin {}", spec.name);
        if spec.exec {
            units.push(Unit::new(origin, Place::Start, vec![format!("  {call}")]));
        } else {
            // An `out` gives the value a reader, so nothing prunes the gate. A
            // record result is read through its first field instead, and an exec
            // field by a handler.
            let mut forms = vec![format!("out o_{} = {call}", spec.name)];
            if let Some(out) = spec.outputs.iter().find(|o| o.field.is_some()) {
                let field = out.field.unwrap_or_default();
                forms.push(if out.ty == Type::Exec {
                    format!(
                        "let l_{n} = {call}\non l_{n}.{field} {{\n  ev_hits += 1\n}}",
                        n = spec.name
                    )
                } else {
                    format!("out o_{} = {call}.{field}", spec.name)
                });
            }
            units.push(Unit::new(origin, Place::Top, forms));
        }
    }
}

fn method_units(d: &mut Decls, units: &mut Vec<Unit>, skipped: &mut Vec<(String, String)>) {
    for m in wirescript::catalog::arrays::array_methods() {
        let elem = Type::Int;
        let Some(decl) = value_decl(&elem) else {
            continue;
        };
        d.arrays.insert(decl);
        let args: Option<Vec<String>> = m
            .params(&elem)
            .iter()
            .filter(|p| !p.optional)
            .map(|p| arg_expr(&p.ty, true, d))
            .collect();
        match args {
            Some(args) => units.push(Unit::new(
                format!("array method {}", m.name),
                Place::Start,
                vec![format!("  arr_{decl}.{}({})", m.name, args.join(", "))],
            )),
            None => skipped.push((
                format!("array .{}", m.name),
                "no dummy value for a param".into(),
            )),
        }
    }
    for m in wirescript::catalog::maps::map_methods() {
        let args: Option<Vec<String>> = m
            .params(&Type::Int, &Type::Int)
            .iter()
            .filter(|p| !p.optional)
            .map(|p| arg_expr(&p.ty, true, d))
            .collect();
        match args {
            Some(args) => units.push(Unit::new(
                format!("map method {}", m.name),
                Place::Start,
                vec![format!("  map_int.{}({})", m.name, args.join(", "))],
            )),
            None => skipped.push((
                format!("map .{}", m.name),
                "no dummy value for a param".into(),
            )),
        }
    }
}

fn event_units(units: &mut Vec<Unit>) {
    let mut events: Vec<_> = wirescript::catalog::events::events().values().collect();
    events.sort_by_key(|e| e.surface_name);
    for e in events {
        let config: Vec<&str> = e.config_positional.iter().map(|_| "\"x\"").collect();
        let head = if config.is_empty() {
            format!("on {}()", e.surface_name)
        } else {
            format!("on {}({})", e.surface_name, config.join(", "))
        };
        units.push(Unit::new(
            format!("event {}", e.surface_name),
            Place::Top,
            vec![
                format!("{head} {{\n  ev_hits += 1\n}}"),
                format!("on {} {{\n  ev_hits += 1\n}}", e.surface_name),
            ],
        ));
    }
}

/// The gates only a language construct makes.
fn construct_units(units: &mut Vec<Unit>) {
    let top = |units: &mut Vec<Unit>, origin: &str, src: &str| {
        units.push(Unit::new(origin, Place::Top, vec![src.to_string()]))
    };
    let start = |units: &mut Vec<Unit>, origin: &str, src: &str| {
        units.push(Unit::new(origin, Place::Start, vec![src.to_string()]))
    };
    top(units, "buffer", "buffer c_prev = st_int");
    top(units, "outer rerouter pin", "@left in c_side: int");
    top(
        units,
        "fn definition",
        "fn c_add(a: int, b: int = 1) -> int {\n  return a + b\n}",
    );
    top(
        units,
        "named chip",
        "chip CChip(a: int) -> (b: int) {\n  out b = a + 1\n}\nlet c_chip = CChip(st_int)",
    );
    top(units, "anonymous chip", "chip {\n  var c_inner: int = 0\n}");
    top(
        units,
        "custom event receiver",
        "on CustomEvent(\"c_channel\") -> (n: int) {\n  ev_hits += n\n}",
    );
    let binary = [
        ("add", "op_i + op_j"),
        ("sub", "op_i - op_j"),
        ("mul", "op_i * op_j"),
        ("div", "op_i / op_j"),
        ("mod", "op_i % op_j"),
        ("eq", "op_i == op_j"),
        ("ne", "op_i != op_j"),
        ("lt", "op_i < op_j"),
        ("le", "op_i <= op_j"),
        ("gt", "op_i > op_j"),
        ("ge", "op_i >= op_j"),
        ("and", "op_a && op_b"),
        ("or", "op_a || op_b"),
        ("xor", "op_a ^ op_b"),
        ("nand", "!(op_a && op_b)"),
        ("nor", "!(op_a || op_b)"),
        ("not", "!op_a"),
        ("neg", "-op_i"),
        ("band", "op_i & op_j"),
        ("bor", "op_i | op_j"),
        ("bxor", "op_i ^ op_j"),
        ("bnot", "~op_i"),
        ("shl", "op_i << op_j"),
        ("shr", "op_i >> op_j"),
        ("cat", "op_s .. op_t"),
        ("select", "if op_a then op_i else op_j"),
    ];
    for (name, expr) in binary {
        top(
            units,
            &format!("operator {expr}"),
            &format!("out c_{name} = {expr}"),
        );
    }
    for ty in [
        "BRItemBase",
        "BRPickupBase",
        "BRWeaponProjectile",
        "BrickAudioDescriptor",
        "BrickFontDescriptor",
        "BrickOneShotAudioDescriptor",
        "BrickWheelEngineAudioDescriptor",
    ] {
        if let Some(name) = wirescript::analysis::assets::asset_names(ty).first() {
            top(
                units,
                &format!("asset reference {ty}"),
                &format!("out c_ref_{ty} = ${ty}/{name}"),
            );
        }
    }
    top(
        units,
        "enum conversion",
        "let c_prop: BrickProperty = IntToEnum(op_i)\nout c_enum = EnumToInt(c_prop)",
    );
    top(units, "edge trigger", "on op_a {\n  ev_hits += 1\n}");
    start(units, "array element write", "  arr_int[0] = st_int");
    start(
        units,
        "if / else",
        "  if st_int > 0 {\n    st_int = 1\n  } else {\n    st_int = 2\n  }",
    );
    start(units, "fn call", "  st_int = c_add(st_int)");
    start(
        units,
        "range loop",
        "  for i in 0..3 {\n    st_int += i\n  }",
    );
    start(
        units,
        "array loop",
        "  for v in arr_int {\n    st_int += v\n  }",
    );
    start(
        units,
        "map loop",
        "  for k, v in map_int {\n    st_int += v\n  }",
    );
    start(
        units,
        "break",
        "  for i in 0..3 {\n    if i == 1 { break }\n  }",
    );
    start(units, "conversion", "  st_float = st_int as float");
    start(
        units,
        "environment property",
        "  Environment.Sky.TimeOfDay = Environment.Sky.TimeOfDay",
    );
    start(
        units,
        "component property",
        "  let c_light = GetMicrochipBrick().AddComponent($Component/PointLight)\n  c_light.Brightness = c_light.Brightness",
    );
    start(
        units,
        "custom event send",
        "  SendCustomEvent(\"c_channel\", st_int)",
    );
    start(
        units,
        "global custom event send",
        "  SendGlobalCustomEvent(\"c_channel\", st_int)",
    );
    start(units, "await Sleep", "  await Sleep(_, delay = 0.1)");
    start(
        units,
        "await SleepTicks",
        "  await SleepTicks(_, delay = 1)",
    );
}

/// The file, and the unit each line came from.
fn render(units: &[Unit], d: &Decls) -> (String, Vec<Option<usize>>) {
    let mut s = String::new();
    let mut owner: Vec<Option<usize>> = Vec::new();
    let line = |s: &mut String, owner: &mut Vec<Option<usize>>, text: &str, unit: Option<usize>| {
        for l in text.split('\n') {
            s.push_str(l);
            s.push('\n');
            owner.push(unit);
        }
    };
    let header = "// AUTO-GENERATED by examples/gen_every_builtin.rs - do not edit by hand.\n\
                  // Places every gate the language can reach so pasting the compiled\n\
                  // circuit surfaces gate encoding / load bugs. `start` is an unwired\n\
                  // input (never fires); the gates are placed regardless. See COVERAGE.md.\n\
                  @nofold\n\n\
                  in start: exec\n\
                  in src_entity: entity\n\
                  in src_char: character\n\
                  in src_ctrl: controller\n\
                  in src_zone: zone\n\
                  in src_tp: teleport\n\
                  in op_i: int\n\
                  in op_j: int\n\
                  in op_a: bool\n\
                  in op_b: bool\n\
                  in op_s: string\n\
                  in op_t: string\n\
                  var st_int: int = 0\n\
                  var st_float: float = 0.0\n\
                  var ev_hits: int = 0\n\
                  var map_int: Map<int, int>";
    line(&mut s, &mut owner, header, None);
    for a in &d.arrays {
        line(&mut s, &mut owner, &format!("var arr_{a}: {a}[]"), None);
    }
    for r in &d.refs {
        line(
            &mut s,
            &mut owner,
            &format!("var ref_{r}: {r}{}", value_init(r)),
            None,
        );
    }
    line(&mut s, &mut owner, "", None);
    for (i, u) in units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.place == Place::Top)
    {
        line(&mut s, &mut owner, &u.forms[u.at], Some(i));
    }
    line(&mut s, &mut owner, "\non start {", None);
    for (i, u) in units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.place == Place::Start)
    {
        line(&mut s, &mut owner, &u.forms[u.at], Some(i));
    }
    line(&mut s, &mut owner, "}", None);
    (s, owner)
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("usage: gen_every_builtin <out.ws>");
    let out_path = Path::new(&out);
    let mut decls = Decls::default();
    let mut units: Vec<Unit> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    builtin_units(&mut decls, &mut units, &mut skipped);
    method_units(&mut decls, &mut units, &mut skipped);
    event_units(&mut units);
    construct_units(&mut units);

    // Compile; drop (or fall back on) every unit a diagnostic lands in.
    let mut dropped: BTreeMap<String, String> = BTreeMap::new();
    let world = loop {
        let (src, owner) = render(&units, &decls);
        std::fs::write(out_path, &src).expect("write output");
        let result = wirescript::compile_to_world(
            wirescript::CompileInput {
                source: &src,
                file: &out,
                module_name: None,
                fold_mode: wirescript::FoldMode::ForceOff,
            },
            Default::default(),
        );
        let errors = match result {
            Ok(r) => break r.world,
            Err(wirescript::CompileError::HasErrors(e)) => e,
            Err(e) => panic!("compile failed outside the source: {e:?}"),
        };
        let mut hit: BTreeMap<usize, String> = BTreeMap::new();
        for e in &errors {
            match owner
                .get((e.range.start.line as usize).wrapping_sub(1))
                .copied()
                .flatten()
            {
                Some(u) => {
                    hit.entry(u)
                        .or_insert_with(|| format!("{} {}", e.code, e.message));
                }
                None => panic!(
                    "an error outside every unit, line {}: {} {}",
                    e.range.start.line, e.code, e.message
                ),
            }
        }
        for (u, why) in hit.into_iter().rev() {
            if units[u].at + 1 < units[u].forms.len() {
                units[u].at += 1;
            } else {
                dropped.insert(units[u].origin.clone(), why);
                units.remove(u);
            }
        }
    };
    let brz = out_path.with_extension("brz");
    world.write_brz(&brz).expect("write brz");

    // Coverage: the component types the emitted world holds, against the
    // inventory's gate classes.
    let mut placed: BTreeSet<String> = BTreeSet::new();
    for b in world
        .bricks
        .iter()
        .chain(world.grids.iter().flat_map(|(_, b)| b.iter()))
    {
        for c in &b.components {
            if let Some(t) = c.component_type() {
                placed.insert(t.to_string());
            }
        }
    }
    let mut inventory: BTreeMap<String, String> = BTreeMap::new();
    for g in wirescript::catalog::default_catalog().entries() {
        inventory
            .entry(g.component.class.clone())
            .or_insert_with(|| g.component.display_name.clone());
    }
    let covered: Vec<&String> = inventory.keys().filter(|c| placed.contains(*c)).collect();
    let missing: Vec<(&String, &String)> = inventory
        .iter()
        .filter(|(c, _)| !placed.contains(*c))
        .collect();

    let mut md = String::new();
    md.push_str("# Gate coverage of `every_builtin`\n\n");
    md.push_str("Generated by `crates/wirescript/examples/gen_every_builtin.rs` with the save. Do not edit by hand.\n\n");
    md.push_str(&format!(
        "The save places **{} of {}** inventory gate classes. Paste `every_builtin.brz`: a gate that fails to load shows in the game log as \"Failed to connect wire\", \"Missing brick to attach component\" or an unknown component type.\n\nPlace it on an entity (a dynamic grid), not the global grid: the whole-grid events (`WholeGridInteracted`, `WholeGridTargeted`) load only there, and on the global grid the game drops both as overlapping.\n\n",
        covered.len(),
        inventory.len()
    ));
    // A `Component_*` class (other than a microchip shell) is a brick
    // component: a light, a button, a joint. No compile places one; it lives
    // on its own brick, or is added to one at runtime.
    let is_component =
        |c: &str| c.starts_with("Component_") && !c.starts_with("Component_Internal_Microchip");
    let (components, gates): (Vec<_>, Vec<_>) = missing.iter().partition(|(c, _)| is_component(c));
    md.push_str(&format!(
        "## Gates not placed ({})\n\nNo Wirescript form makes these yet, or the generator does not reach them.\n\n| Class | Name |\n|---|---|\n",
        gates.len()
    ));
    for (c, n) in &gates {
        md.push_str(&format!("| `{c}` | {n} |\n"));
    }
    md.push_str(&format!(
        "\n## Brick components not placed ({})\n\nComponents a brick carries rather than gates: no compile places them. `AddComponent` adds one at runtime.\n\n| Class | Name |\n|---|---|\n",
        components.len()
    ));
    for (c, n) in &components {
        md.push_str(&format!("| `{c}` | {n} |\n"));
    }
    md.push_str(&format!(
        "\n## Left out by the generator ({})\n\n| Builtin or construct | Why |\n|---|---|\n",
        skipped.len() + dropped.len()
    ));
    for (n, why) in skipped
        .iter()
        .map(|(n, w)| (n.clone(), w.clone()))
        .chain(dropped.clone())
    {
        md.push_str(&format!(
            "| {n} | {} |\n",
            why.replace('|', "\\|").replace('\n', " ")
        ));
    }
    md.push_str(&format!("\n## Placed ({})\n\n", covered.len()));
    for c in &covered {
        md.push_str(&format!("- `{c}`\n"));
    }
    let report = out_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("COVERAGE.md");
    std::fs::write(&report, md).expect("write report");
    eprintln!(
        "wrote {out}, {}, {}: {} of {} classes placed, {} units, {} left out",
        brz.display(),
        report.display(),
        covered.len(),
        inventory.len(),
        units.len(),
        skipped.len() + dropped.len()
    );
}
