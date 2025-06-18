use std::{collections::HashMap, path::Path, sync::Arc};

use indexmap::IndexMap;
use rusqlite::{Connection, params};

use crate::brdb::{
    errors::{BrdbError, BrdbSchemaError},
    fs::BrdbFs,
    schema::SchemaGlobalData,
    tables::{BrdbFile, BrdbFolder},
};

pub mod errors;
pub mod fs;
pub mod schema;
pub mod tables;

pub struct Brdb {
    conn: Connection,
    fs: fs::BrdbFs,
    global_data: Arc<SchemaGlobalData>,
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

        let data_raw = self.fs.cd("World/0/GlobalData.schema")?.read(self)?;
        let schema = schema::BrdbSchema::read(data_raw.as_slice())?;
        let Some(soa) = schema.get_struct("BRSavedGlobalDataSoA") else {
            Err(BrdbSchemaError::MissingStruct(
                "BRSavedGlobalDataSoA".to_string(),
            ))?
        };

        todo!("populate global data from the GlobalData.mps file");

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
    let db = Brdb::open("./Parkour.brdb")?;
    let fs = db.tree(None, 0)?;
    let wires_schema = fs.cd("World/0/Bricks/WiresShared.schema")?.read(&db)?;
    println!(
        "wires: {}",
        schema::BrdbSchema::read(wires_schema.as_slice())?
    );
    let components_schema = fs.cd("World/0/Bricks/ComponentsShared.schema")?.read(&db)?;
    println!(
        "components: {}",
        schema::BrdbSchema::read(components_schema.as_slice())?
    );
    let chunks_schema = fs.cd("World/0/Bricks/ChunksShared.schema")?.read(&db)?;
    println!(
        "chunks: {}",
        schema::BrdbSchema::read(chunks_schema.as_slice())?
    );
    let chunks_index_schema = fs.cd("World/0/Bricks/ChunkIndexShared.schema")?.read(&db)?;
    println!(
        "chunk index: {}",
        schema::BrdbSchema::read(chunks_index_schema.as_slice())?
    );
    let global_data_schema = fs.cd("World/0/GlobalData.schema")?.read(&db)?;
    println!(
        "global data: {}",
        schema::BrdbSchema::read(global_data_schema.as_slice())?
    );

    Ok(())
}
