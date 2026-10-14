//! A written save read back, for tests and examples that check what the game
//! loads rather than what the emitter meant to write.

use brdb::schema::BrdbStruct;
use brdb::{BrReader, Brz, ChunkIndex, ComponentChunkSoA, IntoReader};

/// A `.brz` save opened for reading.
pub struct Save {
    reader: BrReader<<Brz as IntoReader>::Inner>,
    grids: Vec<usize>,
}

impl Save {
    /// Open the save `brz` holds. Panics when it is not one.
    pub fn read(brz: &[u8]) -> Save {
        let brz = Brz::read_slice(brz).expect("read brz");
        let folders = &brz.index_data.folder_names;
        let mut grids: Vec<usize> = folders
            .iter()
            .zip(&brz.index_data.folder_parent_ids)
            .filter(|(_, parent)| usize::try_from(**parent).is_ok_and(|p| folders.get(p).is_some_and(|n| n == "Grids")))
            .map(|(name, _)| name.parse().unwrap_or_else(|_| panic!("grid folder {name}")))
            .collect();
        grids.sort_unstable();
        Save { reader: brz.into_reader(), grids }
    }

    /// The id of every grid, ascending.
    pub fn grid_ids(&self) -> &[usize] {
        &self.grids
    }

    /// Every chunk holding components, grid by grid: its grid, its index, its
    /// component table and its components.
    pub fn component_chunks(&self) -> Vec<(usize, ChunkIndex, ComponentChunkSoA, Vec<BrdbStruct>)> {
        let mut out = Vec::new();
        for &gid in &self.grids {
            for chunk in self.reader.brick_chunk_index(gid).expect("read the grid's chunk index") {
                if chunk.num_components == 0 {
                    continue;
                }
                let (soa, comps) = self.reader.component_chunk_soa(gid, chunk.index).expect("read components");
                out.push((gid, chunk.index, soa, comps));
            }
        }
        out
    }

    /// Every component, grid by grid.
    pub fn components(&self) -> Vec<BrdbStruct> {
        self.component_chunks().into_iter().flat_map(|(_, _, _, comps)| comps).collect()
    }

    /// The name of each component type, by type index.
    pub fn component_type_names(&self) -> Vec<String> {
        self.reader.global_data().expect("read global data").component_type_names.iter().cloned().collect()
    }

    /// The name of each wire port, by port index.
    pub fn wire_port_names(&self) -> Vec<String> {
        self.reader.global_data().expect("read global data").component_wire_port_names.iter().cloned().collect()
    }

    /// Every chunk holding wires, grid by grid: its grid, its index and its
    /// wire table.
    pub fn wires(&self) -> Vec<(usize, ChunkIndex, BrdbStruct)> {
        let mut out = Vec::new();
        for &gid in &self.grids {
            for chunk in self.reader.brick_chunk_index(gid).expect("read the grid's chunk index") {
                if chunk.num_wires > 0 {
                    out.push((gid, chunk.index, self.reader.wire_chunk_soa(gid, chunk.index).expect("read wires")));
                }
            }
        }
        out
    }
}
