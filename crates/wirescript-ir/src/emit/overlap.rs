//! Post-emit sweep: no two bricks in one grid may intersect.
//!
//! The game drops an intersecting brick at load, silently, orphaning its components and
//! dangling every wire into it. Wires have a choke point (`EmitContext::add_wire`) but
//! `bricks.push` sites reach the world unguarded, so the finished world is swept instead.
//!
//! Boxes use the compiler's own sizes: the catalog `half_size` for a gate class, the
//! procedural half-extent for a label carrier or procedural gate, `brdb`'s `local_bounds`
//! for other basic bricks, and an x/y swap on a quarter turn.

use brdb::{BrickType, World};

use crate::catalog::{Catalog, default_catalog};

use super::options::EmitError;

/// Half-extent as the COMPILER believes it, in grid units.
fn half_extent(catalog: &Catalog, b: &brdb::Brick) -> [i32; 3] {
    let mut half = match &b.asset {
        // A procedural brick's size is its own: a label carrier, or a
        // procedural gate placed at the variant layout reserved (which the
        // catalog's default half-size below would understate).
        BrickType::Procedural { size, .. } => {
            let h = [i32::from(size.x), i32::from(size.y), i32::from(size.z)];
            return match b.rotation {
                brdb::Rotation::Deg90 | brdb::Rotation::Deg270 => [h[1], h[0], h[2]],
                _ => h,
            };
        }
        BrickType::Basic(_) => {
            let (lo, hi) = b.local_bounds();
            [(hi.x - lo.x) / 2, (hi.y - lo.y) / 2, (hi.z - lo.z) / 2]
        }
    };
    // `component_type_ref` borrows; `component_type` allocates a BString, so it is only reached
    // for the component kinds that lack the borrowing accessor.
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
                .map(std::string::ToString::to_string)
                .or_else(|| c.component_type().map(|s| s.to_string()))
        })
        .collect();
    format!(
        "{:?}@({},{},{}) half{:?} rot{:?} {:?}",
        b.asset, b.position.x, b.position.y, b.position.z, h, b.rotation, comps
    )
}

type Box3 = ([i32; 3], [i32; 3]);

fn intersects(a: &Box3, b: &Box3) -> bool {
    (0..3).all(|k| a.1[k] > b.0[k] && b.1[k] > a.0[k])
}

/// The first pair of intersecting boxes as (earlier, later): taking the boxes
/// in order of their low x, the first that intersects an earlier one, and the
/// earliest of those. On a dense layout the boxes go into grid cells as large
/// as the largest box (`first_overlap_cells`); on a sparse one, where most
/// cells would be empty, an x sweep finds the same pair.
fn first_overlap(boxes: &[Box3]) -> Option<(usize, usize)> {
    let mut order: Vec<usize> = (0..boxes.len()).collect();
    order.sort_unstable_by_key(|&i| boxes[i].0[0]);
    first_overlap_cells(boxes, &order).unwrap_or_else(|| first_overlap_sweep(boxes, &order))
}

/// [`first_overlap`] by an x sweep with an active window: each box is tested
/// against the earlier ones still reaching past its low x, which every earlier
/// box intersecting it does.
fn first_overlap_sweep(boxes: &[Box3], order: &[usize]) -> Option<(usize, usize)> {
    let mut active: Vec<usize> = Vec::new();
    for &i in order {
        let a = boxes[i];
        active.retain(|&j| boxes[j].1[0] > a.0[0]);
        if let Some(&j) = active.iter().find(|&&j| intersects(&a, &boxes[j])) {
            return Some((j, i));
        }
        active.push(i);
    }
    None
}

/// [`first_overlap`] over grid cells, or `None` when the boxes are spread so
/// thinly that the cells would outnumber them eightfold. A cell is as large as
/// the largest box on each axis and a box covers the cells its interior
/// reaches, so two intersecting boxes share a cell. Each cell's boxes are a
/// list threaded through one array.
#[expect(
    clippy::option_option,
    reason = "outer `None` means too sparse for cells; inner is the overlap found"
)]
fn first_overlap_cells(boxes: &[Box3], order: &[usize]) -> Option<Option<(usize, usize)>> {
    if boxes.is_empty() {
        return Some(None);
    }
    let mut size = [1i32; 3];
    for (lo, hi) in boxes {
        for k in 0..3 {
            size[k] = size[k].max(hi[k] - lo[k]);
        }
    }
    let span = |b: &Box3, k: usize| {
        (
            b.0[k].div_euclid(size[k]),
            (b.1[k] - 1)
                .div_euclid(size[k])
                .max(b.0[k].div_euclid(size[k])),
        )
    };
    let (mut min, mut max) = ([i32::MAX; 3], [i32::MIN; 3]);
    for b in boxes {
        for k in 0..3 {
            let (lo, hi) = span(b, k);
            min[k] = min[k].min(lo);
            max[k] = max[k].max(hi);
        }
    }
    let dims: [usize; 3] =
        std::array::from_fn(|k| (i64::from(max[k]) - i64::from(min[k]) + 1) as usize);
    let volume = dims.iter().try_fold(1usize, |v, d| v.checked_mul(*d))?;
    if volume > boxes.len().saturating_mul(8) {
        return None;
    }
    const NONE: u32 = u32::MAX;
    let mut head = vec![NONE; volume];
    // One entry per (box, cell): the box's place in `order`, and the cell's
    // entry before it.
    let mut entry: Vec<(u32, u32)> = Vec::with_capacity(boxes.len());
    for (at, &i) in order.iter().enumerate() {
        let a = &boxes[i];
        let (x, y, z) = (span(a, 0), span(a, 1), span(a, 2));
        let cells = (x.0..=x.1).flat_map(|cx| {
            (y.0..=y.1).flat_map(move |cy| {
                (z.0..=z.1).map(move |cz| {
                    let c = [cx - min[0], cy - min[1], cz - min[2]].map(|v| v as usize);
                    (c[2] * dims[1] + c[1]) * dims[0] + c[0]
                })
            })
        });
        let mut hit: Option<usize> = None;
        for cell in cells.clone() {
            let mut e = head[cell];
            while e != NONE {
                let (before, next) = entry[e as usize];
                let before = before as usize;
                if hit.is_none_or(|h| before < h) && intersects(a, &boxes[order[before]]) {
                    hit = Some(before);
                }
                e = next;
            }
        }
        if let Some(before) = hit {
            return Some(Some((order[before], i)));
        }
        for cell in cells {
            entry.push((at as u32, head[cell]));
            head[cell] = (entry.len() - 1) as u32;
        }
    }
    Some(None)
}

/// Check one grid's brick list.
fn scan(catalog: &Catalog, label: &str, bricks: &[brdb::Brick]) -> Result<(), EmitError> {
    let boxes: Vec<Box3> = bricks
        .iter()
        .map(|b| {
            let h = half_extent(catalog, b);
            let p = b.position;
            (
                [p.x - h[0], p.y - h[1], p.z - h[2]],
                [p.x + h[0], p.y + h[1], p.z + h[2]],
            )
        })
        .collect();
    match first_overlap(&boxes) {
        Some((j, i)) => Err(EmitError::BrickOverlap(format!(
            "{label}: {} intersects {} - the game drops one of them at load, \
             orphaning its components and dangling every wire into it",
            describe(&bricks[j], half_extent(catalog, &bricks[j])),
            describe(&bricks[i], half_extent(catalog, &bricks[i])),
        ))),
        None => Ok(()),
    }
}

/// Every brick in the finished world, per grid. Grids are separate coordinate
/// spaces, so a root brick can never intersect a grid brick.
pub(super) fn check_no_overlap(world: &World) -> Result<(), EmitError> {
    let catalog = default_catalog();
    scan(catalog, "root grid", &world.bricks)?;
    for (i, (entity, bricks)) in world.grids.iter().enumerate() {
        scan(catalog, &format!("grid {i} ({})", entity.asset), bricks)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cell search reports the pair the x sweep reports, on boxes laid
    /// out densely enough for the cells to be used: some sets overlap, some
    /// touch only face to face, some are empty.
    #[test]
    fn cells_find_the_pair_the_sweep_finds() {
        let mut seed = 0x9e37_79b9_u64;
        let mut rand = |n: i32| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) % n as u64) as i32
        };
        let mut used_cells = 0;
        for round in 0..400 {
            let count = 1 + rand(60) as usize;
            let spread = 2 + rand(if round % 2 == 0 { 30 } else { 200 });
            let boxes: Vec<Box3> = (0..count)
                .map(|_| {
                    let lo = [rand(spread) * 2, rand(spread) * 2, rand(spread)];
                    let half = [1 + rand(5), 1 + rand(5), 1 + rand(2)];
                    (
                        lo,
                        [
                            lo[0] + 2 * half[0],
                            lo[1] + 2 * half[1],
                            lo[2] + 2 * half[2],
                        ],
                    )
                })
                .collect();
            let mut order: Vec<usize> = (0..boxes.len()).collect();
            order.sort_unstable_by_key(|&i| boxes[i].0[0]);
            let sweep = first_overlap_sweep(&boxes, &order);
            if let Some(cells) = first_overlap_cells(&boxes, &order) {
                used_cells += 1;
                assert_eq!(cells, sweep, "round {round}: {boxes:?}");
            }
            assert_eq!(first_overlap(&boxes), sweep, "round {round}");
        }
        assert!(
            used_cells > 100,
            "the cells were tried on only {used_cells} sets"
        );
    }
}
