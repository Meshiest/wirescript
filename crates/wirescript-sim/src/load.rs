//! Reading a `.brz` save into its components and wires, in the order the
//! save stores them. Fan-out order is the stored wire order, so nothing here
//! sorts.

use brdb::schema::{BrdbStruct, BrdbValue};
use brdb::{BrError, Brz, ChunkIndex, ComponentChunkSoA, IntoReader, WireChunkSoA};

use crate::collections::HashMap;

/// A brick as a save addresses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BrickAddr {
    pub grid: u32,
    pub chunk: (i16, i16, i16),
    pub brick: u32,
}

impl BrickAddr {
    pub fn new(grid: u32, chunk: ChunkIndex, brick: u32) -> Self {
        Self {
            grid,
            chunk: (chunk.x, chunk.y, chunk.z),
            brick,
        }
    }
}

/// One component instance: the gate on a brick.
pub struct RawComponent {
    pub addr: BrickAddr,
    pub type_index: u16,
    pub class: String,
    /// The component's data struct, or `None` for a type without one (a
    /// rerouter).
    pub data: Option<BrdbStruct>,
}

/// One end of a wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PortAddr {
    pub brick: BrickAddr,
    pub type_index: u16,
    pub port: u16,
}

/// One wire, as stored in its target's chunk.
#[derive(Clone, Copy, Debug)]
pub struct RawWire {
    pub source: PortAddr,
    pub target: PortAddr,
    /// The wire crosses from one grid into another.
    pub remote: bool,
}

/// A save's wire graph as stored.
pub struct RawSave {
    pub type_names: Vec<String>,
    pub port_names: Vec<String>,
    pub components: Vec<RawComponent>,
    /// Every wire: grid by grid ascending, chunk by chunk in the grid's chunk
    /// index order, local wires before remote ones.
    pub wires: Vec<RawWire>,
    /// Microchip brick -> the persistent index of its inner grid.
    pub chip_grids: HashMap<BrickAddr, u32>,
    /// The prefabs the save embeds, by the path a spawner's `Prefab` names
    /// (`Prefabs/Uploads/<hash>.brz`).
    pub prefabs: HashMap<String, Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("not a readable save: {0}")]
    Brz(#[from] brdb::BrzError),
    #[error("reading the save: {0}")]
    Read(#[from] BrError),
    #[error("reading the save: {0}")]
    Schema(#[from] brdb::BrdbSchemaError),
    #[error("malformed save: {0}")]
    Malformed(String),
}

/// Read every component and wire of the `.brz` in `bytes`.
pub fn read_brz(bytes: &[u8]) -> Result<RawSave, LoadError> {
    let brz = Brz::read_slice(bytes)?;
    let grids = grid_ids(&brz)?;
    let reader = brz.into_reader();
    let prefabs = reader.read_prefabs()?.into_iter().collect();
    let global = reader.global_data()?;
    let type_names: Vec<String> = global.component_type_names.iter().cloned().collect();
    if global.component_data_struct_names.len() != type_names.len() {
        return Err(LoadError::Malformed(format!(
            "{} component types but {} data struct names",
            type_names.len(),
            global.component_data_struct_names.len()
        )));
    }
    let port_names: Vec<String> = global.component_wire_port_names.iter().cloned().collect();

    let mut components = Vec::new();
    let mut wires = Vec::new();
    let mut chip_grids = HashMap::default();
    for &gid in &grids {
        for chunk in reader.brick_chunk_index(gid as usize)? {
            if chunk.num_components > 0 {
                let (soa, data) = reader.component_chunk_soa(gid as usize, chunk.index)?;
                read_components(
                    gid,
                    chunk.index,
                    &soa,
                    data,
                    &type_names,
                    &global.component_data_struct_names,
                    &mut components,
                )?;
                if soa.microchip_brick_indices.len() != soa.microchip_brick_grid_references.len() {
                    return Err(LoadError::Malformed(format!(
                        "chunk {} of grid {gid}: microchip bricks and grids differ in length",
                        chunk.index
                    )));
                }
                for (&b, &g) in soa
                    .microchip_brick_indices
                    .iter()
                    .zip(&soa.microchip_brick_grid_references)
                {
                    chip_grids.insert(BrickAddr::new(gid, chunk.index, b), g);
                }
            }
            if chunk.num_wires > 0 {
                let s = reader.wire_chunk_soa(gid as usize, chunk.index)?;
                let soa = WireChunkSoA::try_from(&BrdbValue::Struct(Box::new(s)))?;
                read_wires(gid, chunk.index, &soa, &mut wires)?;
            }
        }
    }
    Ok(RawSave {
        type_names,
        port_names,
        components,
        wires,
        chip_grids,
        prefabs,
    })
}

/// The persistent index of every grid folder, ascending.
fn grid_ids(brz: &Brz) -> Result<Vec<u32>, LoadError> {
    let folders = &brz.index_data.folder_names;
    let mut grids = Vec::new();
    for (name, parent) in folders.iter().zip(&brz.index_data.folder_parent_ids) {
        let under_grids = usize::try_from(*parent)
            .is_ok_and(|p| folders.get(p).is_some_and(|n| n == "Grids"));
        if under_grids {
            let id = name
                .parse()
                .map_err(|_| LoadError::Malformed(format!("grid folder {name:?}")))?;
            grids.push(id);
        }
    }
    grids.sort_unstable();
    grids.dedup();
    Ok(grids)
}

/// A chunk's components in stored order: grouped by type, instances in order
/// within a type. A type whose data struct is `None` stores no data.
fn read_components(
    grid: u32,
    chunk: ChunkIndex,
    soa: &ComponentChunkSoA,
    data: Vec<BrdbStruct>,
    type_names: &[String],
    struct_names: &[String],
    out: &mut Vec<RawComponent>,
) -> Result<(), LoadError> {
    let mut data = data.into_iter();
    let mut bricks = soa.component_brick_indices.iter();
    for counter in &soa.component_type_counters {
        let ty = counter.type_index as usize;
        let class = type_names
            .get(ty)
            .ok_or_else(|| LoadError::Malformed(format!("component type index {ty}")))?;
        let has_data = struct_names.get(ty).is_some_and(|s| s != "None");
        for _ in 0..counter.num_instances {
            let brick = *bricks
                .next()
                .ok_or_else(|| LoadError::Malformed("component without a brick".into()))?;
            let data = if has_data {
                Some(
                    data.next()
                        .ok_or_else(|| LoadError::Malformed(format!("{class} without data")))?,
                )
            } else {
                None
            };
            out.push(RawComponent {
                addr: BrickAddr::new(grid, chunk, brick),
                type_index: u16::try_from(counter.type_index).map_err(|_| {
                    LoadError::Malformed(format!("component type index {}", counter.type_index))
                })?,
                class: class.clone(),
                data,
            });
        }
    }
    if bricks.next().is_some() || data.next().is_some() {
        return Err(LoadError::Malformed(format!(
            "chunk {chunk} of grid {grid}: components and their data differ in count"
        )));
    }
    Ok(())
}

fn read_wires(
    grid: u32,
    chunk: ChunkIndex,
    soa: &WireChunkSoA,
    out: &mut Vec<RawWire>,
) -> Result<(), LoadError> {
    if soa.local_wire_sources.len() != soa.local_wire_targets.len()
        || soa.remote_wire_sources.len() != soa.remote_wire_targets.len()
    {
        return Err(LoadError::Malformed(format!(
            "wire chunk {chunk} of grid {grid}: sources and targets differ in length"
        )));
    }
    let target = |t: &brdb::WirePortTarget| PortAddr {
        brick: BrickAddr::new(grid, chunk, t.brick_index_in_chunk),
        type_index: t.component_type_index,
        port: t.port_index,
    };
    for (s, t) in soa.local_wire_sources.iter().zip(&soa.local_wire_targets) {
        out.push(RawWire {
            source: PortAddr {
                brick: BrickAddr::new(grid, chunk, s.brick_index_in_chunk),
                type_index: s.component_type_index,
                port: s.port_index,
            },
            target: target(t),
            remote: false,
        });
    }
    for (s, t) in soa.remote_wire_sources.iter().zip(&soa.remote_wire_targets) {
        out.push(RawWire {
            source: PortAddr {
                brick: BrickAddr::new(s.grid_persistent_index, s.chunk_index, s.brick_index_in_chunk),
                type_index: s.component_type_index,
                port: s.port_index,
            },
            target: target(t),
            remote: s.grid_persistent_index != grid,
        });
    }
    Ok(())
}
