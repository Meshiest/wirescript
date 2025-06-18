use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use indexmap::{IndexMap, IndexSet};
use rusqlite::{Connection, params};

use crate::brdb::{
    errors::{BrdbError, BrdbSchemaError},
    fs::BrdbFs,
    schema::BrdbSchemaGlobalData,
    tables::{BrdbFile, BrdbFolder},
};

pub mod errors;
pub mod fs;
pub mod schema;
pub mod tables;

pub struct Brdb {
    conn: Connection,
    fs: fs::BrdbFs,
    global_data: Arc<BrdbSchemaGlobalData>,
}

pub const REQUIRED_TABLES: [&'static str; 4] = ["blobs", "revisions", "folders", "files"];

impl Brdb {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BrdbError> {
        let conn = Connection::open(path)?;

        // Ensure all the brdb tables exist
        for t in &REQUIRED_TABLES {
            if !conn.table_exists(None, *t)? {
                return Err(BrdbError::MissingTable(t));
            }
        }

        Ok(Self {
            conn,
            // Empty root fs by default
            fs: fs::BrdbFs::Root(Default::default()),
            global_data: Default::default(),
        })
    }

    pub fn populate(&mut self) -> Result<(), BrdbError> {
        self.fs = self.tree(None, 0)?;

        // Parse the GlobalData schem
        let schema_data = self.fs.cd("World/0/GlobalData.schema")?.read(self)?;
        let schema = Arc::new(schema::BrdbSchema::read(schema_data.as_slice())?);

        // Parse the GlobalData struct of arrays
        let mps_data = self.fs.cd("World/0/GlobalData.mps")?.read(self)?;
        let mps =
            schema::BrdbValue::read_type(&schema, "BRSavedGlobalDataSoA", &mut &mps_data[..])?;

        let mps_struct = mps.as_struct()?;

        let str_set = |prop| {
            mps_struct
                .prop(prop)?
                .as_array()?
                .into_iter()
                .map(|s| Ok(s.as_str()?.to_owned()))
                .collect::<Result<IndexSet<String>, BrdbSchemaError>>()
        };

        // Extract the asset names and types from the data
        let mut external_asset_types = HashSet::new();
        let external_asset_references = mps_struct
            .prop("ExternalAssetReferences")?
            .as_array()?
            .into_iter()
            .map(|s| {
                let s = s.as_struct()?;
                let asset_type = s.prop("PrimaryAssetType")?.as_str()?;
                let asset_name = s.prop("PrimaryAssetName")?.as_str()?;
                external_asset_types.insert(asset_type.to_owned());
                Ok((asset_type.to_owned(), asset_name.to_owned()))
            })
            .collect::<Result<IndexSet<_>, BrdbSchemaError>>()?;

        self.global_data = Arc::new(BrdbSchemaGlobalData {
            external_asset_types,
            external_asset_references,
            entity_type_names: str_set("EntityTypeNames")?,
            basic_brick_asset_names: str_set("BasicBrickAssetNames")?,
            procedural_brick_asset_names: str_set("ProceduralBrickAssetNames")?,
            material_asset_names: str_set("MaterialAssetNames")?,
            component_type_names: str_set("ComponentTypeNames")?,
            component_data_struct_names: str_set("ComponentDataStructNames")?,
            component_wire_port_names: str_set("ComponentWirePortNames")?,
        });

        Ok(())
    }

    pub fn sqlite_schema(&self) -> Result<String, BrdbError> {
        let schemas = self
            .conn
            .prepare("SELECT sql FROM sqlite_schema")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(format!("{}", schemas.join("\n")))
    }

    pub fn tree(&self, parent: Option<BrdbFolder>, depth: usize) -> Result<BrdbFs, BrdbError> {
        let (parent_query, params) = if let Some(p) = parent.as_ref() {
            ("= ?1", params![p.folder_id])
        } else {
            ("IS NULL", params![])
        };
        let dirs = self
            .conn
            .prepare(&format!(
                "SELECT name, folder_id, parent_id, created_at, deleted_at
                FROM folders
                WHERE parent_id {parent_query} AND deleted_at IS NULL
                ORDER BY name;"
            ))?
            .query_map(params, |row| {
                Ok(BrdbFolder {
                    name: row.get(0)?,
                    folder_id: row.get(1)?,
                    parent_id: row.get(2)?,
                    created_at: row.get(3)?,
                    deleted_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut children = IndexMap::new();

        for dir in dirs {
            children.insert(dir.name.clone(), self.tree(Some(dir), depth + 1)?);
        }

        if let Some(parent) = parent.as_ref() {
            let files = self
                .conn
                .prepare(
                    "SELECT name, file_id, parent_id, content_id, created_at, deleted_at
                    FROM files
                    WHERE parent_id = ?1 AND deleted_at IS NULL
                    ORDER BY name;",
                )?
                .query_map(params![parent.folder_id], |row| {
                    let name: String = row.get(0)?;
                    Ok((
                        name.clone(),
                        BrdbFs::File(BrdbFile {
                            name,
                            file_id: row.get(1)?,
                            parent_id: row.get(2)?,
                            content_id: row.get(3)?,
                            created_at: row.get(4)?,
                            deleted_at: row.get(5)?,
                        }),
                    ))
                })?
                .collect::<Result<HashMap<_, _>, _>>()?;
            children.extend(files);
        }

        Ok(match parent {
            Some(p) => BrdbFs::Folder(p, children),
            None => BrdbFs::Root(children),
        })
    }
}

#[test]
fn test() -> Result<(), BrdbError> {
    let mut db = Brdb::open("./Parkour.brdb")?;
    let fs = db.tree(None, 0)?;
    // let wires_schema = fs.cd("World/0/Bricks/WiresShared.schema")?.read(&db)?;
    // println!(
    //     "wires: {}",
    //     schema::BrdbSchema::read(wires_schema.as_slice())?
    // );
    // let components_schema = fs.cd("World/0/Bricks/ComponentsShared.schema")?.read(&db)?;
    // println!(
    //     "components: {}",
    //     schema::BrdbSchema::read(components_schema.as_slice())?
    // );
    // let chunks_schema = fs.cd("World/0/Bricks/ChunksShared.schema")?.read(&db)?;
    // println!(
    //     "chunks: {}",
    //     schema::BrdbSchema::read(chunks_schema.as_slice())?
    // );
    // let chunks_index_schema = fs.cd("World/0/Bricks/ChunkIndexShared.schema")?.read(&db)?;
    // println!(
    //     "chunk index: {}",
    //     schema::BrdbSchema::read(chunks_index_schema.as_slice())?
    // );
    let global_data_schema = fs.cd("World/0/GlobalData.schema")?.read(&db)?;
    println!(
        "global data: {}",
        schema::BrdbSchema::read(global_data_schema.as_slice())?
    );

    // Troubleshooting reading data
    // let mps_data = fs.cd("World/0/GlobalData.mps")?.read(&db)?;
    // let mut cursor = std::io::Cursor::new(mps_data);
    // println!("len {:?}", rmp::decode::read_array_len(&mut cursor));
    // println!("str {:?}", read_owned_str(&mut cursor));
    // let len = rmp::decode::read_array_len(&mut cursor).unwrap();
    // println!("len2 {len}");
    // for _ in 0..len {
    //     println!("str {:?}", read_owned_str(&mut cursor));
    // }
    // println!("marker {:?}", rmp::decode::read_marker(&mut cursor));

    db.populate()?;
    println!(
        "{}",
        db.global_data
            .external_asset_references
            .iter()
            .map(|(ty, name)| format!("{ty} {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!(
        "entity_type_names {}",
        db.global_data
            .entity_type_names
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!(
        "wire port names {}",
        db.global_data
            .component_wire_port_names
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );

    Ok(())
}
