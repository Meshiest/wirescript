//! `@layout("code")` rotates the exec gates on a line's left spine a
//! quarter-turn so they point down the chain.
//!
//! A rotated brick's footprint swaps its x and y half-sizes. The layout
//! decides the rotation and reserves the swapped cell; emit must apply the
//! identical swap when it centers the brick. Nothing downstream catches a
//! disagreement: `brdb::Brick::local_bounds()` is rotation-blind, so the
//! bricks would pass every other checker and be dropped by the game at load.
//! These tests are the gate for that contract.

use crate::common;

use std::collections::HashMap;

use wirescript::catalog::default_catalog;
use wirescript::emit::EmitOptions;
use wirescript::{CompileInput, compile_to_world};

/// `DisplayText` is a statement sink (an exec input, nothing on its line reads
/// its value back), so it is pinned to that line's leftmost column and rotates.
/// It is 8x5, so the swap moves real geometry (a 16x10 footprint becomes
/// 10x16); square gates could not tell a correct swap from a missing one.
///
/// The same line carries the unrotated `Var_Get` and `FormatText` that build
/// its text, in the value columns to its right, so an under-reserved cell puts
/// the wide brick through a real neighbour.
///
/// The own-line comments matter for the overlap sweep: each emits an invisible
/// `PB_DefaultBrick` carrier on a row of its own.
const SRC: &str = "@layout(\"code\")\n\
                   \n\
                   // a counter, bumped twice per join\n\
                   var a: int = 1\n\
                   on ControllerJoined() -> (c) {\n\
                     // the first bump\n\
                     a = a + 1\n\
                     c.DisplayText(\"hi ${a}\")\n\
                     a = a + 2\n\
                   }\n";

/// Brick asset -> the catalog half-size the layout reserves for it. This is
/// the only footprint source that agrees with the layout; brdb's
/// `local_bounds()` neither knows these sizes nor applies rotation.
fn half_sizes_by_asset() -> HashMap<String, (i32, i32)> {
    let mut out: HashMap<String, (i32, i32)> = HashMap::new();
    for g in default_catalog().entries() {
        let hs = (g.half_size.x, g.half_size.y);
        out.entry(g.brick_asset.clone())
            .and_modify(|e| {
                // One asset backing two half-sizes: take the larger so the
                // sweep stays conservative.
                e.0 = e.0.max(hs.0);
                e.1 = e.1.max(hs.1);
            })
            .or_insert(hs);
    }
    out
}

/// Brick asset -> half-size for the overlap sweep: the catalog, plus the one
/// brick emit places that no gate catalog knows about.
///
/// `PB_DefaultBrick` is the invisible 1x1 procedural plate (half-extents 5, 5)
/// under a comment annotation and under a plane's header text. The game drops
/// one of two overlapping bricks at load, so it must be measured too.
fn overlap_half_sizes() -> HashMap<String, (i32, i32)> {
    let mut out = half_sizes_by_asset();
    out.insert("PB_DefaultBrick".to_string(), (5, 5));
    out
}

/// The footprint a brick actually occupies, as `(half_x, half_y)`, with
/// the swap applied for a quarter-turned brick.
///
/// Only quarter turns swap: `Deg180` measures like `Deg0`. Every rotation is
/// matched explicitly so a newly added facing must be classified.
fn brick_half_size(brick: &brdb::Brick, sizes: &HashMap<String, (i32, i32)>) -> Option<(i32, i32)> {
    // A procedural gate (`PB_Gate_*`) is placed at a variant size of its own,
    // which the per-asset catalog default would understate.
    let known = *sizes.get(brick.asset.asset().as_ref())?;
    let (hx, hy) = match &brick.asset {
        brdb::BrickType::Procedural { size, .. } => (i32::from(size.x), i32::from(size.y)),
        brdb::BrickType::Basic(_) => known,
    };
    Some(match brick.rotation {
        brdb::Rotation::Deg90 | brdb::Rotation::Deg270 => (hy, hx),
        brdb::Rotation::Deg0 | brdb::Rotation::Deg180 => (hx, hy),
    })
}

#[test]
fn leftmost_exec_bricks_are_emitted_rotated() {
    let r = compile_to_world(common::input(SRC, "rotation.ws"), EmitOptions::default())
        .expect("should compile");

    let rotated: Vec<&brdb::Brick> = r
        .world
        .grids
        .iter()
        .flat_map(|(_, bricks)| bricks.iter())
        .chain(r.world.bricks.iter())
        .filter(|b| matches!(b.rotation, brdb::Rotation::Deg90))
        .collect();
    assert!(
        !rotated.is_empty(),
        "a code-layout build must emit at least one quarter-turned exec brick"
    );

    // The wide Sweep gate is among them, so the swap is load-bearing here.
    let sizes = half_sizes_by_asset();
    assert!(
        rotated.iter().any(|b| {
            sizes
                .get(&b.asset.asset().to_string())
                .is_some_and(|(hx, hy)| hx != hy)
        }),
        "fixture must rotate a NON-SQUARE gate, or the footprint swap is untested"
    );
}

#[test]
fn emitted_brick_corners_match_the_cells_layout_reserved() {
    let r = compile_to_world(common::input(SRC, "rotation.ws"), EmitOptions::default())
        .expect("should compile");

    let sizes = half_sizes_by_asset();
    // `r.layout.placements` is the root module's layout, the map emit read.
    // A brick's min corner is its center minus its (possibly swapped)
    // half-extent, so recomputing it must reproduce the reserved cells; a swap
    // mismatch between layout and emit shifts that brick's corner alone.
    //
    // Emit translates a grid's contents as a block when it centers the plane,
    // so both sets are normalised by their own minimum first.
    //
    // Gutter bus rerouters have no `NodeId` and so no entry in `placements`;
    // their reserved cells come off the layout's bus and meet the same contract.
    let mut reserved: Vec<(i32, i32, i32)> = r
        .layout
        .placements
        .values()
        .map(|p| (p.x, p.y, p.z))
        .collect();
    assert!(!reserved.is_empty(), "fixture must place some gates");
    reserved.extend(r.layout.bus.nodes.iter().map(|n| (n.x, n.y, n.z)));

    // The fixture has no chips, so exactly one grid carries the root
    // module's gate bricks.
    let corners: Vec<(i32, i32, i32)> = r
        .world
        .grids
        .iter()
        .flat_map(|(_, bricks)| bricks.iter())
        .filter_map(|b| {
            // Skip non-catalog bricks (microchip shell, plane header, labels).
            let (hx, hy) = brick_half_size(b, &sizes)?;
            Some((b.position.x - hx, b.position.y - hy, b.position.z))
        })
        .collect();
    assert_eq!(
        corners.len(),
        reserved.len(),
        "every placed gate must emit exactly one catalog brick"
    );

    fn normalise(v: &[(i32, i32, i32)]) -> Vec<(i32, i32, i32)> {
        let mx = v.iter().map(|p| p.0).min().unwrap();
        let my = v.iter().map(|p| p.1).min().unwrap();
        let mz = v.iter().map(|p| p.2).min().unwrap();
        let mut out: Vec<(i32, i32, i32)> =
            v.iter().map(|p| (p.0 - mx, p.1 - my, p.2 - mz)).collect();
        out.sort_unstable();
        out
    }

    assert_eq!(
        normalise(&corners),
        normalise(&reserved),
        "every emitted gate brick's min corner must land on the cell the layout \
         reserved; a mismatch means emit's rotation offset disagrees with the \
         layout's footprint reservation"
    );
}

/// A name label is brick-local: it rides the brick's yaw. A quarter-turned
/// gate therefore has to carry a counter-rotated label, or its variable tag
/// reads a quarter-turn off from every other tag on the plane.
///
/// The fixture's `spin` name appears twice: full size (2.4) on the unrotated
/// `Pseudo_Var` brick, and as the small tag (1.2) on the rotated exec gate that
/// writes it, so one source pins both sides of the rule.
#[test]
fn a_rotated_gates_label_is_counter_rotated() {
    use brdb::schema::BrdbValue;

    /// `(text, line height)` -> the label rotations emitted for it.
    fn labels(src: &str, file: &str) -> Vec<(String, f32, f32)> {
        let cr = wirescript::compile::compile(CompileInput::new(src, file))
            .expect("should compile to brz");
        let mut out: Vec<(String, f32, f32)> = Vec::new();
        for c in common::world_components(&cr.brz) {
            let (
                Some(BrdbValue::String(text)),
                Some(BrdbValue::F32(line_height)),
                Some(BrdbValue::F32(rotation)),
            ) = (c.get("Text"), c.get("LineHeight"), c.get("Rotation"))
            else {
                continue;
            };
            out.push((text.clone(), *line_height, *rotation));
        }
        out.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out
    }

    let body = "var spin: int = 0\nin go: exec\non go { spin = spin + 1 }\n";
    let coded = labels(&format!("@layout(\"code\")\n\n{body}"), "spun.ws");
    // The only `spin` tag at 1.2 is the rotated gate's own: a lane brick
    // carries colour and nothing else, so the bus contributes no text.
    let tag = |ls: &[(String, f32, f32)], h: f32| -> f32 {
        let hits: Vec<f32> = ls
            .iter()
            .filter(|(t, lh, _)| t == "spin" && *lh == h)
            .map(|(_, _, r)| *r)
            .collect();
        assert!(
            !hits.is_empty(),
            "expected a `spin` label at size {h}: {ls:?}"
        );
        assert!(
            hits.iter().all(|r| *r == hits[0]),
            "every `spin` label at size {h} must share one angle: {ls:?}"
        );
        hits[0]
    };

    // The gate that writes `spin` is on the exec spine, so it is turned.
    assert_eq!(
        tag(&coded, 1.2),
        -135.0,
        "a rotated gate's variable tag must take the brick's 90° yaw back \
         out so it reads at the same angle as every other tag"
    );
    // The variable brick beside it is not, and keeps the plain angle.
    assert_eq!(
        tag(&coded, 2.4),
        -45.0,
        "an unrotated brick's label is uncompensated"
    );

    // Without the code layout nothing rotates, so no tag is compensated.
    let dag = labels(body, "spun_dag.ws");
    assert_eq!(tag(&dag, 1.2), -45.0);
    assert_eq!(tag(&dag, 2.4), -45.0);
}

#[test]
fn no_emitted_bricks_overlap_with_rotation_aware_footprints() {
    let r = compile_to_world(common::input(SRC, "rotation.ws"), EmitOptions::default())
        .expect("should compile");

    let sizes = overlap_half_sizes();
    // The carriers must be present, or measuring them proves nothing.
    let carriers = r
        .world
        .grids
        .iter()
        .flat_map(|(_, bricks)| bricks.iter())
        .filter(|b| b.asset.asset().to_string() == "PB_DefaultBrick")
        .count();
    assert!(
        carriers > 0,
        "fixture must emit comment/header carrier bricks for the sweep to measure"
    );
    for (gi, (_, bricks)) in r.world.grids.iter().enumerate() {
        let boxes: Vec<(String, i32, i32, i32, i32, i32)> = bricks
            .iter()
            .filter_map(|b| {
                let (hx, hy) = brick_half_size(b, &sizes)?;
                Some((
                    b.asset.asset().to_string(),
                    b.position.x - hx,
                    b.position.x + hx,
                    b.position.y - hy,
                    b.position.y + hy,
                    b.position.z,
                ))
            })
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let disjoint = a.5 != b.5 || a.2 <= b.1 || b.2 <= a.1 || a.4 <= b.3 || b.4 <= a.3;
                assert!(
                    disjoint,
                    "grid {gi}: {} [{}..{}]x[{}..{}] overlaps {} [{}..{}]x[{}..{}] \
                     under rotation-aware footprints",
                    a.0, a.1, a.2, a.3, a.4, b.0, b.1, b.2, b.3, b.4
                );
            }
        }
    }
}
