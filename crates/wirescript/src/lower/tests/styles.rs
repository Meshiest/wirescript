//! `@color` / `@material`: parsing, checking, the IR forms, the `.wsir` text
//! and the Brick Property Changer in the emitted save.

use crate::ast::{Stmt, TopDecl};
use crate::intern::sym;
use crate::ir::{Literal, Module, Node, NodeKind, StyleValue};

const CHANGER: &str = "Component_BrickPropertyChanger";

fn front(src: &str) -> crate::compile::FrontEnd {
    crate::compile::front_end(
        src,
        "styles.ws",
        None,
        crate::lower::FoldMode::Auto,
        &crate::resolve::FsLoader,
    )
}

/// Every diagnostic code `src` produces, parse to lowering.
fn codes(src: &str) -> Vec<String> {
    let r = super::lower_with(
        src,
        "styles.ws",
        crate::lower::FoldMode::Auto,
        &crate::resolve::FsLoader,
    );
    r.diagnostics.iter().map(|d| d.code.clone()).collect()
}

/// Every parse diagnostic message `src` produces.
fn parse_errors(src: &str) -> Vec<String> {
    crate::parser::parse(src, "styles.ws")
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// `src` lowered, with no error anywhere on the way.
fn lowered(src: &str) -> Module {
    super::compile_ok_auto(src).module
}

fn var_node<'a>(m: &'a Module, name: &str) -> &'a Node {
    m.nodes
        .values()
        .find(|n| matches!(n.properties.get(&*sym::NAME_LABEL), Some(Literal::String(s)) if s == name))
        .unwrap_or_else(|| panic!("no var `{name}`"))
}

fn chip_node(m: &Module) -> &Node {
    m.nodes
        .values()
        .find(|n| n.kind == NodeKind::Chip)
        .expect("a chip node")
}

fn glow() -> i64 {
    crate::catalog::enum_member_value("EBrickMaterial", "Glow").expect("EBrickMaterial has Glow")
}

#[test]
fn parses_on_a_var_a_chip_and_the_module() {
    let src = "@color(Color(1.0, 0.0, 0.0))\n@material(Glow)\n\n\
               @color(Color(0.0, 1.0, 0.0)) @label(\"v\")\nvar a: int = 0\n\
               @material(Metal)\nchip { @color(Color(0.0, 0.0, 1.0)) var b: int = 0 }\n\
               @material(Glass) chip C() { }\n";
    let r = crate::parser::parse(src, "styles.ws");
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    assert!(r.ast.module_color.is_some() && r.ast.module_material.is_some());
    assert!(r.ast.module_label.is_none());
    let mut seen = (false, false, false, false);
    for d in &r.ast.decls {
        match d {
            TopDecl::Var(v) => {
                seen.0 = v.color_expr.is_some()
                    && v.material_expr.is_none()
                    && v.label.as_deref() == Some("v");
            }
            TopDecl::AnonChip(ac) => {
                seen.1 = ac.material_expr.is_some() && ac.color_expr.is_none();
                seen.2 = ac
                    .body
                    .stmts
                    .iter()
                    .any(|s| matches!(s, Stmt::Var(v) if v.color_expr.is_some()));
            }
            TopDecl::Chip(c) => seen.3 = c.material_expr.is_some(),
            _ => {}
        }
    }
    assert_eq!(seen, (true, true, true, true));
}

#[test]
fn a_run_without_a_blank_line_styles_the_declaration() {
    let r = crate::parser::parse(
        "@label(\"x\")\n@color(Color(1.0, 0.0, 0.0))\nvar a: int = 0\n",
        "styles.ws",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    assert!(r.ast.module_color.is_none() && r.ast.module_label.is_none());
    assert!(
        matches!(&r.ast.decls[0], TopDecl::Var(v) if v.color_expr.is_some() && v.label.is_some())
    );
}

#[test]
fn a_module_run_follows_the_other_module_annotations() {
    let r = crate::parser::parse(
        "@invisible\n@label(\"t\") @material(Glow)\n\nvar a: int = 0\n",
        "styles.ws",
    );
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
    assert!(r.ast.invisible && r.ast.module_label.is_some() && r.ast.module_material.is_some());
}

#[test]
fn duplicates_are_parse_errors() {
    let dup = |src: &str, word: &str| {
        let errors = parse_errors(src);
        assert!(
            errors.iter().any(|e| e == &format!("duplicate @{word}")),
            "{src:?}: {errors:?}"
        );
    };
    dup(
        "@color(Color(1.0, 0.0, 0.0)) @color(Color(0.0, 1.0, 0.0))\nvar a: int = 0\n",
        "color",
    );
    dup("@material(Glow)\n@material(Metal)\nchip { }\n", "material");
    dup(
        "@material(Glow)\n@material(Metal)\n\nvar a: int = 0\n",
        "material",
    );
    dup(
        "chip { @color(Color(1.0, 0.0, 0.0)) @color(Color(1.0, 0.0, 0.0)) var a: int = 0 }\n",
        "color",
    );
}

#[test]
fn misplaced_annotations_are_parse_errors() {
    for src in [
        "@color(Color(1.0, 0.0, 0.0))\nin x: int\n",
        "@material(Glow)\nout y = 1\n",
        "chip { @color(Color(1.0, 0.0, 0.0)) in x: int }\n",
        "@material(Glow)\nmod m(v: int) -> int { return v }\n",
        "@color(Color(1.0, 0.0, 0.0))\nfn f(v: int) -> int { return v }\n",
        "@color(Color(1.0, 0.0, 0.0))\nlet k = 1\n",
        "in go: exec\non go { @material(Glow) for i in 0..3 { } }\n",
        "@color\nvar a: int = 0\n",
    ] {
        assert!(
            !parse_errors(src).is_empty(),
            "{src:?} must not parse cleanly"
        );
    }
}

#[test]
fn values_of_the_wrong_type_are_ws003() {
    for src in [
        "@color(5)\nvar a: int = 0\n",
        "@color(BrickMaterial.Glow)\nvar a: int = 0\n",
        "@material(Color(1.0, 0.0, 0.0))\nvar a: int = 0\n",
        "@material(3)\nchip { }\n",
        "var n: int = 0\n@color(n)\nvar a: int = 0\n",
        "@material(1.5)\n\nvar a: int = 0\n",
    ] {
        assert!(
            codes(src).contains(&"WS003".to_string()),
            "{src:?}: {:?}",
            codes(src)
        );
    }
}

#[test]
fn a_runtime_value_off_the_top_level_is_ws040() {
    for src in [
        "var h: float = 0.0\nchip { @color(Color(h, 0.0, 0.0)) var a: int = 0 }\n",
        "var m: BrickMaterial = BrickMaterial.Glow\n@material(m)\nchip { }\n",
        "chip C(h: float) { @color(Color(h, 0.0, 0.0)) var a: int = 0 }\nlet c = C(1.0)\n",
    ] {
        assert!(
            codes(src).contains(&"WS040".to_string()),
            "{src:?}: {:?}",
            codes(src)
        );
    }
}

#[test]
fn a_constant_bakes_as_node_properties() {
    let m = lowered(
        "let K = 20\n\
         @color(ColorSRGB(200, 10, K, 255)) @material(Glow)\nvar a: int = 0\n\
         @material(BrickMaterial.Metal)\nchip { var b: int = 0 }\n\
         @color(Color(1.0, 0.5, 0.0))\nvar c: int = 0\n",
    );
    let a = var_node(&m, "a");
    assert_eq!(
        a.properties.get(&*sym::BRICK_COLOR),
        Some(&Literal::Color {
            r: 200,
            g: 10,
            b: 20,
            a: 255
        })
    );
    assert_eq!(
        a.properties.get(&*sym::BRICK_MATERIAL),
        Some(&Literal::Int(glow()))
    );
    // A linear color is stored as the sRGB bytes a brick holds.
    assert_eq!(
        var_node(&m, "c").properties.get(&*sym::BRICK_COLOR),
        Some(&Literal::Color {
            r: 255,
            g: 188,
            b: 0,
            a: 255
        })
    );
    let metal = crate::catalog::enum_member_value("EBrickMaterial", "Metal").unwrap();
    let chip = chip_node(&m);
    assert_eq!(
        chip.properties.get(&*sym::BRICK_MATERIAL),
        Some(&Literal::Int(metal))
    );
    assert!(chip.properties.get(&*sym::BRICK_COLOR).is_none());
    assert!(
        m.dynamic_colors.is_empty() && m.dynamic_materials.is_empty() && m.root_style.is_empty()
    );
}

#[test]
fn a_named_chip_bakes_on_each_instance() {
    let m = lowered(
        "@color(Color(0.0, 0.0, 1.0))\nchip C(x: int) -> (r: int) { out r = x }\nlet one = C(1)\nlet two = C(2)\nout o = one.r + two.r\n",
    );
    let chips: Vec<&Node> = m
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::Chip)
        .collect();
    assert_eq!(chips.len(), 2);
    for c in chips {
        assert_eq!(
            c.properties.get(&*sym::BRICK_COLOR),
            Some(&Literal::Color {
                r: 0,
                g: 0,
                b: 255,
                a: 255
            })
        );
    }
}

#[test]
fn a_record_var_styles_every_field_gate() {
    let m =
        lowered("type P = { x: int, y: int }\n@material(Hologram)\nvar p: P = { x: 1, y: 2 }\n");
    let styled = m
        .nodes
        .values()
        .filter(|n| n.properties.contains_key(&*sym::BRICK_MATERIAL))
        .count();
    assert_eq!(styled, 2);
}

#[test]
fn a_runtime_value_is_recorded_for_its_host() {
    let m = lowered(
        "var hue: float = 0.0\nvar kind: BrickMaterial = BrickMaterial.Plastic\n\
         @color(Color(hue, 0.0, 0.0)) @material(kind)\nvar lit: int = 0\n\
         in go: exec\non go { hue = hue + 0.1; kind = BrickMaterial.Glow }\n",
    );
    let lit = var_node(&m, "lit");
    let color = m
        .dynamic_colors
        .get(&lit.id)
        .expect("a runtime @color source");
    assert_eq!(
        m.nodes[&color.node_id].output_type(color.port),
        Some(crate::ir::Type::Color)
    );
    let material = m
        .dynamic_materials
        .get(&lit.id)
        .expect("a runtime @material source");
    assert_eq!(
        m.nodes[&material.node_id].output_type(material.port),
        Some(crate::ir::Type::Int)
    );
    assert!(lit.properties.get(&*sym::BRICK_COLOR).is_none());
}

#[test]
fn the_module_style_is_baked_or_wired() {
    let m = lowered(
        "@color(ColorSRGB(1, 2, 3, 255))\n@material(kind)\n\nvar kind: BrickMaterial = BrickMaterial.Glass\n",
    );
    assert_eq!(
        m.root_style.color,
        Some(StyleValue::Baked(Literal::Color {
            r: 1,
            g: 2,
            b: 3,
            a: 255
        }))
    );
    assert!(matches!(m.root_style.material, Some(StyleValue::Wired(_))));
}

#[test]
fn an_unstyled_program_carries_no_style() {
    let m = lowered("@label(\"t\")\n\nvar a: int = 0\nchip { var b: int = 0 }\n");
    assert!(
        m.dynamic_colors.is_empty() && m.dynamic_materials.is_empty() && m.root_style.is_empty()
    );
    let mut any = false;
    m.walk(&mut |_, sub| {
        any |= sub.nodes.values().any(|n| {
            n.properties.contains_key(&*sym::BRICK_COLOR)
                || n.properties.contains_key(&*sym::BRICK_MATERIAL)
        })
    });
    assert!(!any);
}

/// A `BrickMaterial` member is bare only as a `@material` argument, so the
/// names it shares with `TextMaterial` still resolve there as before.
#[test]
fn material_members_are_bare_only_inside_the_annotation() {
    let fe = front("let t = Plastic\nout o = t.Discriminant\n");
    assert!(
        fe.typecheck.diagnostics.is_empty(),
        "{:?}",
        fe.typecheck.diagnostics
    );
    assert!(codes("let h = Hologram\nout o = h.Discriminant\n").contains(&"WS002".to_string()));
}

const STYLED: &str = "@color(Color(hue, 0.0, 0.0))\n@material(Glow)\n\n\
                      var hue: float = 0.0\n\
                      @color(ColorSRGB(10, 20, 30, 255))\nvar a: int = 0\n\
                      var kind: BrickMaterial = BrickMaterial.Metal\n\
                      @material(kind)\nvar b: int = 0\n\
                      @material(Hologram)\nchip { var c: int = 0 }\n\
                      in go: exec\non go { hue = hue + 0.1; kind = BrickMaterial.Glass }\n";

fn input(src: &str) -> crate::CompileInput<'_> {
    crate::CompileInput {
        source: src,
        file: "styles.ws",
        module_name: None,
        fold_mode: crate::FoldMode::Auto,
    }
}

#[test]
fn the_text_form_round_trips_and_assembles_the_same_save() {
    let result = crate::compile_to_world(input(STYLED), crate::emit::EmitOptions::default())
        .expect("compiles");
    let direct = result.world.to_brz_vec().expect("encode");
    let ir_path = std::path::Path::new("styles.wsir");
    let mut program = result.program;
    crate::prepare_program_for_text(&mut program, "styles.ws", ir_path, crate::FoldMode::Auto)
        .expect("prepare");
    let text = crate::wsir::program_to_string(&program).expect("write");
    for needle in [
        "color_from = ",
        "material = ",
        "_color = #0a141eff",
        "!material(",
        "_material = ",
    ] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    let parsed = crate::wsir::parse_program(&text, "styles.wsir").unwrap_or_else(|e| panic!("{e}"));
    assert!(parsed.validate().is_empty(), "{:?}", parsed.validate());
    crate::wsir::check_fixed_point(&parsed, &text).unwrap_or_else(|e| panic!("{e}"));
    let assembled = crate::assemble(
        &parsed,
        &crate::assemble_options("styles.wsir", crate::FoldMode::Auto),
    )
    .expect("assemble")
    .world
    .to_brz_vec()
    .expect("encode");
    assert!(
        direct == assembled,
        "the save assembled from the text differs from the direct one"
    );
}

/// The bricks carrying a Brick Property Changer, by id.
fn changer_bricks(world: &brdb::World) -> Vec<&brdb::Brick> {
    world
        .bricks
        .iter()
        .chain(world.grids.iter().flat_map(|(_, b)| b.iter()))
        .filter(|b| {
            b.components
                .iter()
                .any(|c| c.component_type().is_some_and(|t| t.to_string() == CHANGER))
        })
        .collect()
}

#[test]
fn constants_style_the_brick_and_runtime_values_add_a_wired_changer() {
    let result = crate::compile_to_world(input(STYLED), crate::emit::EmitOptions::default())
        .expect("compiles");
    let world = &result.world;
    // Only the root shell (runtime `@color`) and `b` (runtime `@material`)
    // need a changer; `a` and the chip are styled on the brick itself.
    let styled = changer_bricks(world);
    assert_eq!(styled.len(), 2, "{} bricks with a changer", styled.len());
    assert!(
        styled
            .iter()
            .any(|b| b.get_id() == world.bricks[0].get_id()),
        "the root shell has a changer"
    );
    let wires: Vec<(usize, String)> = world
        .wires
        .iter()
        .filter(|w| w.target.component_type.to_string() == CHANGER)
        .map(|w| (w.target.brick_id, w.target.port_name.to_string()))
        .collect();
    assert_eq!(wires.len(), 2, "{wires:?}");
    assert!(
        wires
            .iter()
            .any(|(id, port)| Some(*id) == world.bricks[0].get_id() && port == "Color"),
        "{wires:?}"
    );
    assert!(
        wires
            .iter()
            .any(|(id, port)| Some(*id) != world.bricks[0].get_id() && port == "Material"),
        "{wires:?}"
    );

    let all: Vec<&brdb::Brick> = world
        .bricks
        .iter()
        .chain(world.grids.iter().flat_map(|(_, b)| b.iter()))
        .collect();
    let plain: Vec<&brdb::Brick> = all
        .iter()
        .copied()
        .filter(|b| !styled.iter().any(|s| s.get_id() == b.get_id()))
        .collect();
    // `a`'s constant color and the chip's constant material are the bricks' own.
    assert!(
        plain
            .iter()
            .any(|b| (b.color.r, b.color.g, b.color.b) == (10, 20, 30)),
        "a's color on its brick"
    );
    assert!(
        plain
            .iter()
            .any(|b| b.material == brdb::assets::materials::HOLOGRAM),
        "the chip's material on its brick"
    );
    // The root's constant material is on the shell brick as well as its changer.
    assert_eq!(world.bricks[0].material, brdb::assets::materials::GLOW);

    let comps =
        wirescript_ir::readback::Save::read(&world.to_brz_vec().expect("encode")).components();
    let changers: Vec<String> = comps
        .iter()
        .filter(|c| c.get_name() == "BrickComponentData_BrickPropertyChanger")
        .map(|c| {
            format!("{c}")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_eq!(changers.len(), 2, "{changers:?}");
    for c in &changers {
        assert!(
            c.contains("bVisibility: true") && c.contains("bCollisionEnabled_Physics: true"),
            "{c}"
        );
    }
}

#[test]
fn a_changer_starts_from_the_bricks_own_values() {
    // A constant color with a runtime material: the color is set on the brick,
    // and the changer copies it so only the wired material changes.
    let src = "var kind: BrickMaterial = BrickMaterial.Metal
               @color(ColorSRGB(10, 20, 30, 255)) @material(kind)
var b: int = 0
               in go: exec
on go { kind = BrickMaterial.Glass }
";
    let result =
        crate::compile_to_world(input(src), crate::emit::EmitOptions::default()).expect("compiles");
    let brick = changer_bricks(&result.world)
        .into_iter()
        .next()
        .expect("a brick with a changer")
        .clone();
    assert_eq!(
        (brick.color.r, brick.color.g, brick.color.b),
        (10, 20, 30),
        "the constant color is the brick's"
    );
    let comps = wirescript_ir::readback::Save::read(&result.world.to_brz_vec().expect("encode"))
        .components();
    let changer = comps
        .iter()
        .find(|c| c.get_name() == "BrickComponentData_BrickPropertyChanger")
        .map(|c| {
            format!("{c}")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .expect("a changer");
    for (field, v) in [("R", 10), ("G", 20), ("B", 30)] {
        assert!(
            changer.contains(&format!("{field}: {v}u8")),
            "{field} {v}: {changer}"
        );
    }
    assert!(
        changer.contains(&format!("MaterialAlpha: {}u8", brick.material_intensity)),
        "{changer}"
    );
}

#[test]
fn a_runtime_source_shared_with_another_use_survives_the_merge() {
    // The same pure expression elsewhere is merged into one gate; the style's
    // source must follow it to the gate that stays, or emit drops the wire.
    let cases = [
        (
            "@left in n: int\n@material(BrickMaterial.FromInt(n % 6))\nvar a: int = 0\n@right out s = a.Value\n@right out m = n % 6\n",
            "Material",
        ),
        (
            "@left in hot: bool\n@color(if hot then Color(1.0, 0.0, 0.0) else Color(0.0, 0.0, 1.0))\nvar a: bool = false\n@right out s = a.Value\n@right out c = if hot then Color(1.0, 0.0, 0.0) else Color(0.0, 0.0, 1.0)\n",
            "Color",
        ),
    ];
    for (src, port) in cases {
        let result = crate::compile_to_world(input(src), crate::emit::EmitOptions::default())
            .expect("compiles");
        let wired = result
            .world
            .wires
            .iter()
            .filter(|w| {
                w.target.component_type.to_string() == CHANGER
                    && w.target.port_name.to_string() == port
            })
            .count();
        assert_eq!(wired, 1, "{port} wire for:\n{src}");
    }
}
