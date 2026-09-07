//! Post-emit sweep: no two bricks in one grid may intersect.
//!
//! The game DROPS an intersecting brick at load, silently, orphaning its
//! components and dangling every wire into it (`labels.rs:140-143`, pinned in
//! game). That is the same load-failure class as a wire fan-in, and fan-in has
//! a choke point (`EmitContext::add_wire`) while this had none: six unguarded
//! `bricks.push` sites reach the world and nothing swept them.
//!
//! The box is the one `layout/code/tests.rs::assert_no_overlap` adjudicates
//! with, moved to the emitted side: the compiler's OWN catalog half_size per
//! gate class, the procedural half-extent for a label carrier, and a
//! quarter-turn x/y swap — the same swap `emit/module.rs` applies when it
//! centres the brick. `brdb::Brick::local_bounds()` is NOT usable here: it
//! ignores rotation (`brdb .../brick.rs:142`) and its asset table does not
//! know 44 of the catalog's classes.

use super::*;

/// Half-extent as the COMPILER believes it, in grid units.
fn half_extent(catalog: &crate::catalog::Catalog, b: &brdb::Brick) -> [i32; 3] {
    let mut half = match &b.asset {
        BrickType::Procedural { size, .. } => [size.x as i32, size.y as i32, size.z as i32],
        BrickType::Basic(_) => {
            let (lo, hi) = b.local_bounds();
            [(hi.x - lo.x) / 2, (hi.y - lo.y) / 2, (hi.z - lo.z) / 2]
        }
    };
    // `component_type_ref` borrows; `component_type` allocates a BString, so
    // it is only reached for the handful of component kinds that lack the
    // borrowing accessor. One String per brick here cost 6% of build_world's
    // whole allocation count on 2raab.
    for c in &b.components {
        let found = match c.component_type_ref() {
            Some(ct) => catalog.find_by_class(ct),
            None => match c.component_type() {
                Some(ct) => catalog.find_by_class(ct.as_ref()),
                None => None,
            },
        };
        if let Some(g) = found {
            half = [g.half_size.x, g.half_size.y, g.half_size.z];
            break;
        }
    }
    match b.rotation {
        brdb::Rotation::Deg90 | brdb::Rotation::Deg270 => [half[1], half[0], half[2]],
        _ => half,
    }
}

fn describe(b: &brdb::Brick, h: [i32; 3]) -> String {
    let comps: Vec<String> = b
        .components
        .iter()
        .filter_map(|c| {
            c.component_type_ref()
                .map(|s| s.to_string())
                .or_else(|| c.component_type().map(|s| s.to_string()))
        })
        .collect();
    format!(
        "{:?}@({},{},{}) half{:?} rot{:?} {:?}",
        b.asset, b.position.x, b.position.y, b.position.z, h, b.rotation, comps
    )
}

/// Sweep one grid's brick list. x-sorted with an active window, so this stays
/// linear-ish on the 100k-brick programs `@layout("code")` produces; the
/// O(n^2) double loop `examples/check_overlaps` uses is 5e9 pairs there.
fn scan(
    catalog: &crate::catalog::Catalog,
    label: &str,
    bricks: &[brdb::Brick],
) -> Result<(), EmitError> {
    let mut boxes: Vec<([i32; 3], [i32; 3])> = Vec::with_capacity(bricks.len());
    for b in bricks {
        let h = half_extent(catalog, b);
        let p = b.position;
        boxes.push((
            [p.x - h[0], p.y - h[1], p.z - h[2]],
            [p.x + h[0], p.y + h[1], p.z + h[2]],
        ));
    }
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_unstable_by_key(|&i| boxes[i].0[0]);
    let mut active: Vec<usize> = Vec::new();
    for &i in &order {
        let a = boxes[i];
        active.retain(|&j| boxes[j].1[0] > a.0[0]);
        for &j in &active {
            let b = boxes[j];
            if (0..3).all(|k| a.1[k] > b.0[k] && b.1[k] > a.0[k]) {
                return Err(EmitError::BrickOverlap(format!(
                    "{label}: {} intersects {} - the game drops one of them at load, \
                     orphaning its components and dangling every wire into it",
                    describe(&bricks[j], half_extent(catalog, &bricks[j])),
                    describe(&bricks[i], half_extent(catalog, &bricks[i])),
                )));
            }
        }
        active.push(i);
    }
    Ok(())
}

/// Every brick in the finished world, per grid. Grids are separate coordinate
/// spaces, so a root brick can never intersect a grid brick.
pub(super) fn check_no_overlap(world: &World) -> Result<(), EmitError> {
    let catalog = crate::catalog::default_catalog();
    scan(catalog, "root grid", &world.bricks)?;
    for (i, (entity, bricks)) in world.grids.iter().enumerate() {
        scan(catalog, &format!("grid {i} ({})", entity.asset), bricks)?;
    }
    Ok(())
}
