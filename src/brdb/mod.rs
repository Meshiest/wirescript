use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    ops::Deref,
    path::Path,
    sync::{Arc, RwLock},
};

use indexmap::{IndexMap, IndexSet};
use rusqlite::{Connection, params};

use crate::brdb::{
    assets::LiteralComponent,
    errors::{BrdbError, BrdbFsError, BrdbSchemaError},
    fs::{BrdbFs, now},
    pending::BrdbPendingFs,
    schema::{
        BrdbSchema, BrdbSchemaGlobalData, BrdbStruct, BrdbValue, ReadBrdbSchema,
        as_brdb::AsBrdbValue,
    },
    schemas::BRICK_COMPONENT_SOA,
    tables::{BrdbBlob, BrdbFile, BrdbFolder},
    wrapper::schemas::{
        BRICK_CHUNK_INDEX_SOA, BRICK_CHUNK_SOA, ENTITY_CHUNK_INDEX_SOA, GLOBAL_DATA_SOA,
        OWNER_TABLE_SOA,
    },
};

pub mod assets;
pub mod errors;
pub mod fs;
pub mod pending;
pub mod schema;
pub mod tables;
mod wrapper;
pub use wrapper::*;

pub struct Brdb {
    conn: Connection,
}

pub const REQUIRED_TABLES: [&str; 4] = ["blobs", "revisions", "folders", "files"];
pub const BRDB_SQLITE_SCHEMA: &str = include_str!("./brdb.sql");

impl Brdb {
    /// Open a new in-memory BRDB database.
    pub fn new_memory() -> Result<Self, BrdbError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(BRDB_SQLITE_SCHEMA)?;
        let db = Self { conn };
        db.ensure_tables_exist()?;
        db.create_revision("Initial Revision", now())?;
        Ok(db)
    }

    /// Create a new BRDB database at the specified path.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, BrdbError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(BRDB_SQLITE_SCHEMA)?;
        let db = Self { conn };
        db.ensure_tables_exist()?;
        db.create_revision("Initial Revision", now())?;
        Ok(db)
    }

    pub fn into_reader(self) -> BrdbReader {
        BrdbReader::new(self)
    }

    /// Open an existing BRDB database at the specified path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BrdbError> {
        let db = Self {
            conn: Connection::open(path)?,
        };
        db.ensure_tables_exist()?;
        Ok(db)
    }

    /// Create or open a BRDB database at the specified path.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, BrdbError> {
        let path = path.as_ref();
        if path.exists() {
            Self::open(path)
        } else {
            Self::create(path)
        }
    }

    /// Write a pending operation to the BRDB filesystem.
    pub fn write_pending(
        &self,
        description: impl AsRef<str>,
        pending: BrdbPendingFs,
    ) -> Result<(), BrdbError> {
        let fs = self.get_fs()?;
        fs.write_pending(description.as_ref(), self, pending, Some(14))?;
        Ok(())
    }

    /// Save a world to the BRDB database.
    pub fn save(&self, description: impl AsRef<str>, world: &World) -> Result<(), BrdbError> {
        self.write_pending(description.as_ref(), world.to_unsaved()?.to_pending()?)?;
        Ok(())
    }

    /// Ensure that all required tables exist in the database.
    fn ensure_tables_exist(&self) -> Result<(), BrdbError> {
        for t in &REQUIRED_TABLES {
            if !self.conn.table_exists(None, *t)? {
                return Err(BrdbError::MissingTable(t));
            }
        }
        Ok(())
    }

    /// Read the GlobalData
    pub fn read_global_data(&self) -> Result<Arc<BrdbSchemaGlobalData>, BrdbError> {
        // Parse the GlobalData schema
        let schema = self
            .read_file("World/0/GlobalData.schema")?
            .as_slice()
            .read_brdb_schema()
            .map_err(|e| e.wrap("Read GlobalData Schema"))?;

        // Parse the GlobalData struct of arrays
        let mps = self
            .read_file("World/0/GlobalData.mps")?
            .as_slice()
            .read_brdb(&schema, GLOBAL_DATA_SOA)
            .map_err(|e| e.wrap("Read BRSavedGlobalDataSoA"))?;

        let mps_struct = mps.as_struct()?;

        let str_set = |prop| {
            mps_struct
                .prop(prop)?
                .as_array()?
                .into_iter()
                .map(|s| Ok(s.as_str()?.to_owned()))
                .collect::<Result<IndexSet<String>, BrdbSchemaError>>()
        };
        let str_vec = |prop| {
            mps_struct
                .prop(prop)?
                .as_array()?
                .into_iter()
                .map(|s| Ok(s.as_str()?.to_owned()))
                .collect::<Result<Vec<String>, BrdbSchemaError>>()
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

        Ok(Arc::new(BrdbSchemaGlobalData {
            external_asset_types,
            external_asset_references,
            entity_type_names: str_set("EntityTypeNames")?,
            basic_brick_asset_names: str_set("BasicBrickAssetNames")?,
            procedural_brick_asset_names: str_set("ProceduralBrickAssetNames")?,
            material_asset_names: str_set("MaterialAssetNames")?,
            component_type_names: str_set("ComponentTypeNames")?,
            component_data_struct_names: str_vec("ComponentDataStructNames")?,
            component_wire_port_names: str_set("ComponentWirePortNames")?,
        }))
    }

    /// Obtain the SQLite schema of the BRDB database as a string.
    pub fn sqlite_schema(&self) -> Result<String, BrdbError> {
        let schemas = self
            .conn
            .prepare("SELECT sql FROM sqlite_schema")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(format!("{}", schemas.join("\n")))
    }

    /// Get the filesystem representation of the BRDB database.
    pub fn get_fs(&self) -> Result<BrdbFs, BrdbError> {
        self.tree(None, 0)
    }

    fn tree(&self, parent: Option<BrdbFolder>, depth: usize) -> Result<BrdbFs, BrdbError> {
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

    /// Find and read a file from the brdb filesystem, returning its decompressed content as a byte vector.
    pub fn read_file(&self, path: impl Display) -> Result<Vec<u8>, BrdbFsError> {
        let path = path.to_string();

        if path.starts_with("/") {
            return Err(BrdbFsError::AbsolutePathNotAllowed);
        }

        let mut components = path.split("/").peekable();
        let mut entire_path = String::from("");
        let mut parent_id = None;
        let mut content_id = 0;

        while let Some(name) = components.next() {
            entire_path.push('/');
            entire_path.push_str(name);

            // If there is more in the path, the current component must be a folder
            if components.peek().is_some() {
                let Some(next) = self
                    .find_folder(parent_id, name)
                    .map_err(|e| e.wrap(format!("find folder {entire_path}")))?
                else {
                    return Err(BrdbFsError::NotFound(format!("folder {entire_path}")));
                };
                parent_id = Some(next);
                continue;
            }

            // Find the file in the current folder
            content_id = self
                .find_file(parent_id, name)
                .map_err(|e| e.wrap(format!("find file {entire_path}")))?
                .ok_or_else(|| BrdbFsError::NotFound(format!("file {entire_path}")))?;
            break;
        }

        // Read the blob
        Ok(self
            .find_blob(content_id)
            .map_err(|e| e.wrap(format!("find blob {content_id}")))?
            .read()
            .map_err(|e| e.wrap(format!("read blob {content_id}")))?)
    }

    /// Find a file by its name and parent folder id in the brdb filesystem, returning its folder_id
    pub fn find_folder(
        &self,
        parent_id: Option<i64>,
        name: &str,
    ) -> Result<Option<i64>, BrdbFsError> {
        let res = self.conn.query_one(
            format!(
                "SELECT folder_id FROM folders WHERE {} AND name = ?1 AND deleted_at IS NULL;",
                match parent_id {
                    Some(parent_id) => format!("parent_id = {parent_id}"),
                    None => "parent_id IS NULL".to_owned(),
                }
            )
            .as_str(),
            params![name],
            |row| row.get(0),
        );
        match res {
            Ok(folder_id) => Ok(Some(folder_id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(BrdbFsError::Sqlite(e)),
        }
    }

    /// Find a file by its name and parent in the brdb filesystem, returns the blob_id if found.
    pub fn find_file(
        &self,
        parent_id: Option<i64>,
        name: &str,
    ) -> Result<Option<i64>, BrdbFsError> {
        let res = self.conn.query_one(
            format!(
                "SELECT content_id FROM files WHERE {} AND name = ?1 AND deleted_at IS NULL;",
                match parent_id {
                    Some(parent_id) => format!("parent_id = {parent_id}"),
                    None => "parent_id IS NULL".to_owned(),
                }
            )
            .as_str(),
            params![name],
            |row| row.get(0),
        );
        match res {
            Ok(file_id) => Ok(Some(file_id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(BrdbFsError::Sqlite(e)),
        }
    }

    /// Read the metadata for a file in the brdb filesystem.
    pub fn find_blob(&self, content_id: i64) -> Result<BrdbBlob, BrdbFsError> {
        let res = self
        .conn
        .query_one(
            "SELECT blob_id, compression, size_uncompressed, size_compressed, delta_base_id, hash, content
            FROM blobs
            WHERE blob_id = ?1;",
            params![content_id],
            |row| {
                Ok(BrdbBlob {
                    blob_id: row.get(0)?,
                    compression: row.get(1)?,
                    size_uncompressed: row.get(2)?,
                    size_compressed: row.get(3)?,
                    delta_base_id: row.get(4)?,
                    hash: row.get(5)?,
                    content: row.get(6)?,
                })
            })?;
        Ok(res)
    }

    /// Insert a new folder into the database.
    pub fn insert_folder(
        &self,
        name: &str,
        parent_id: Option<i64>,
        created_at: i64,
    ) -> Result<i64, BrdbFsError> {
        self.conn.execute(
            "INSERT INTO folders (name, parent_id, created_at)
            VALUES (?1, ?2, ?3);",
            params![name, parent_id, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Insert a new file into the database, linking it to a content blob.
    pub fn insert_file(
        &self,
        name: &str,
        parent_id: Option<i64>,
        content_id: i64,
        created_at: i64,
    ) -> Result<i64, BrdbFsError> {
        self.conn.execute(
            "INSERT INTO files (name, parent_id, content_id, created_at)
            VALUES (?1, ?2, ?3, ?4);",
            params![name, parent_id, content_id, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Insert a new blob into the database, compressing it if a zstd level is specified.
    pub fn insert_blob(
        &self,
        mut content: Vec<u8>,
        hash: Vec<u8>,
        zstd_level: Option<i32>,
    ) -> Result<i64, BrdbFsError> {
        let size_uncompressed = content.len() as i64;
        let mut size_compressed = 0;
        let mut compression = 0;

        // Compress the content if a zstd level is specified
        if let Some(zstd_level) = zstd_level {
            let compressed =
                BrdbBlob::compress(&content, zstd_level).map_err(BrdbFsError::Compress)?;
            size_compressed = compressed.len() as i64;
            if size_compressed < size_uncompressed {
                compression = 1;
                content = compressed;
            }
        }

        self.insert_blob_row(BrdbBlob {
            blob_id: -1,
            compression,
            size_uncompressed,
            size_compressed,
            delta_base_id: None,
            hash,
            content,
        })
    }

    /// Insert a new blob into the database, ignoring the id
    pub fn insert_blob_row(&self, blob: BrdbBlob) -> Result<i64, BrdbFsError> {
        self.conn.execute(
            "INSERT INTO blobs (compression, size_uncompressed, size_compressed, delta_base_id, hash, content)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6);",
            params![
                blob.compression,
                blob.size_uncompressed,
                blob.size_compressed,
                blob.delta_base_id,
                blob.hash,
                blob.content
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Check if a blob with the given hash exists in the database.
    pub fn find_blob_by_hash(
        &self,
        size: usize,
        hash: &[u8],
    ) -> Result<Option<BrdbBlob>, BrdbFsError> {
        let res = self.conn
            .query_one(
                "SELECT blob_id, compression, size_uncompressed, size_compressed, delta_base_id, hash, content
                FROM blobs
                WHERE hash = ?1 AND size_uncompressed = ?2;",
                params![hash, size],
                |row| {
                    Ok(BrdbBlob {
                        blob_id: row.get(0)?,
                        compression: row.get(1)?,
                        size_uncompressed: row.get(2)?,
                        size_compressed: row.get(3)?,
                        delta_base_id: row.get(4)?,
                        hash: row.get(5)?,
                        content: row.get(6)?,
                    })
                },
            );
        match res {
            Ok(blob) => Ok(Some(blob)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(BrdbFsError::Sqlite(e)),
        }
    }

    /// Create a new revision in the database with the given description and timestamp.
    pub fn create_revision(&self, description: &str, created_at: i64) -> Result<i64, BrdbFsError> {
        self.conn.execute(
            "INSERT INTO revisions (description, created_at)
            VALUES (?1, ?2);",
            params![description, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Mark a file as deleted by setting its `deleted_at` timestamp.
    pub fn delete_file(&self, file_id: i64, deleted_at: i64) -> Result<(), BrdbFsError> {
        self.conn.execute(
            "UPDATE files SET deleted_at = ?2 WHERE file_id = ?1;",
            params![file_id, deleted_at],
        )?;
        Ok(())
    }

    /// Mark a folder as deleted by setting its `deleted_at` timestamp.
    pub fn delete_folder(&self, folder_id: i64, deleted_at: i64) -> Result<(), BrdbFsError> {
        self.conn.execute(
            "UPDATE folders SET deleted_at = ?2 WHERE folder_id = ?1;",
            params![folder_id, deleted_at],
        )?;
        Ok(())
    }
}

pub struct BrdbReader {
    brdb: Brdb,
    global_data: RwLock<Option<Arc<BrdbSchemaGlobalData>>>,
    owners_schema: RwLock<Option<Arc<BrdbSchema>>>,
    components_schema: RwLock<Option<Arc<BrdbSchema>>>,
    wires_schema: RwLock<Option<Arc<BrdbSchema>>>,
    bricks_schema: RwLock<Option<Arc<BrdbSchema>>>,
    brick_chunk_index_schema: RwLock<Option<Arc<BrdbSchema>>>,
    entity_chunk_index_schema: RwLock<Option<Arc<BrdbSchema>>>,
    entities_schema: RwLock<Option<Arc<BrdbSchema>>>,
}
impl Deref for BrdbReader {
    type Target = Brdb;

    fn deref(&self) -> &Self::Target {
        &self.brdb
    }
}

impl BrdbReader {
    pub fn new(brdb: Brdb) -> Self {
        Self {
            brdb,
            global_data: Default::default(),
            owners_schema: Default::default(),
            components_schema: Default::default(),
            wires_schema: Default::default(),
            bricks_schema: Default::default(),
            brick_chunk_index_schema: Default::default(),
            entity_chunk_index_schema: Default::default(),
            entities_schema: Default::default(),
        }
    }

    /// Read the GlobalData
    pub fn global_data(&self) -> Result<Arc<BrdbSchemaGlobalData>, BrdbError> {
        if let Some(data) = self.global_data.read().unwrap().as_ref() {
            return Ok(data.clone());
        }
        let data = self.read_global_data()?;
        self.global_data.write().unwrap().replace(data.clone());
        Ok(data)
    }
    /// Read the Owners table
    pub fn owners_soa(&self) -> Result<BrdbStruct, BrdbError> {
        let owners_schema = self.owners_schema()?;
        let owners_data = self
            .read_file("World/0/Owners.mps")?
            .as_slice()
            .read_brdb(&owners_schema, OWNER_TABLE_SOA)?;
        match owners_data {
            BrdbValue::Struct(s) => Ok(*s),
            ty => Err(BrdbError::Schema(BrdbSchemaError::ExpectedType(
                "Struct".to_string(),
                ty.get_type().to_owned(),
            ))),
        }
    }
    /// Read the Owners schema
    pub fn owners_schema(&self) -> Result<Arc<BrdbSchema>, BrdbError> {
        if let Some(schema) = self.owners_schema.read().unwrap().as_ref() {
            return Ok(schema.clone());
        }
        let schema = self
            .read_file("World/0/Owners.schema")?
            .as_slice()
            .read_brdb_schema()?;
        self.owners_schema.write().unwrap().replace(schema.clone());
        Ok(schema)
    }
    /// Read the shared components chunk schema
    pub fn components_schema(&self) -> Result<Arc<BrdbSchema>, BrdbError> {
        if let Some(schema) = self.components_schema.read().unwrap().as_ref() {
            return Ok(schema.clone());
        }
        let schema = self
            .read_file("World/0/Bricks/ComponentsShared.schema")?
            .as_slice()
            .read_brdb_schema_with_data(self.global_data()?)?;
        self.components_schema
            .write()
            .unwrap()
            .replace(schema.clone());
        Ok(schema)
    }
    /// Read the shared component chunk indices
    pub fn component_chunk_soa(
        &self,
        grid_id: usize,
        chunk: ChunkIndex,
    ) -> Result<(BrdbStruct, Vec<BrdbStruct>), BrdbError> {
        let global_data = self.global_data()?;
        let schema = self.components_schema()?;
        println!("[debug] component schema: {schema}");

        let path = format!("World/0/Bricks/Grids/{grid_id}/Components/{chunk}.mps");
        let buf = self.read_file(path)?;
        let buf = &mut buf.as_slice();

        let mps = buf.read_brdb(&schema, BRICK_COMPONENT_SOA)?;
        let soa = match mps {
            BrdbValue::Struct(s) => *s,
            ty => {
                return Err(BrdbError::Schema(BrdbSchemaError::ExpectedType(
                    "Struct".to_string(),
                    ty.get_type().to_owned(),
                )));
            }
        };

        let mut component_data = Vec::new();
        let type_counters = soa.prop("ComponentTypeCounters")?.as_array()?;
        for counter in type_counters {
            let type_idx = counter.prop("TypeIndex")?.as_brdb_u32()?;
            let num_instances = counter.prop("NumInstances")?.as_brdb_u32()?;
            let type_name = global_data
                .component_type_names
                .get_index(type_idx as usize)
                .cloned()
                .unwrap_or("illegal".to_string());
            let struct_name = global_data
                .component_data_struct_names
                .get(type_idx as usize)
                .cloned()
                .unwrap_or("illegal".to_string());

            if struct_name == "None" {
                continue;
            }

            for _ in 0..num_instances {
                let BrdbValue::Struct(s) = buf
                    .read_brdb(&schema, &struct_name)
                    .map_err(|e| e.wrap(format!("Read component {type_name}/{struct_name}")))?
                else {
                    continue;
                };
                component_data.push(*s);
            }
        }
        Ok((soa, component_data))
    }
    /// Read the shared wires chunk schema
    pub fn wires_schema(&self) -> Result<Arc<BrdbSchema>, BrdbError> {
        if let Some(schema) = self.wires_schema.read().unwrap().as_ref() {
            return Ok(schema.clone());
        }
        let schema = self
            .read_file("World/0/Bricks/WiresShared.schema")?
            .as_slice()
            .read_brdb_schema()?;
        self.wires_schema.write().unwrap().replace(schema.clone());
        Ok(schema)
    }
    pub fn wire_chunk_soa(
        &self,
        grid_id: usize,
        chunk: ChunkIndex,
    ) -> Result<BrdbStruct, BrdbError> {
        let path = format!("World/0/Bricks/Grids/{grid_id}/Wires/{chunk}.mps");
        let mps = self
            .read_file(path)?
            .as_slice()
            .read_brdb(&self.wires_schema()?, BRICK_CHUNK_SOA)?;
        match mps {
            BrdbValue::Struct(s) => Ok(*s),
            ty => Err(BrdbError::Schema(BrdbSchemaError::ExpectedType(
                "Struct".to_string(),
                ty.get_type().to_owned(),
            ))),
        }
    }
    /// Read the shared brick-chunk-index schema
    pub fn brick_chunk_index_schema(&self) -> Result<Arc<BrdbSchema>, BrdbError> {
        if let Some(schema) = self.brick_chunk_index_schema.read().unwrap().as_ref() {
            return Ok(schema.clone());
        }
        let schema = self
            .read_file("World/0/Bricks/ChunkIndexShared.schema")?
            .as_slice()
            .read_brdb_schema()?;
        self.brick_chunk_index_schema
            .write()
            .unwrap()
            .replace(schema.clone());
        Ok(schema)
    }
    /// Read the shared bricks chunk schema
    pub fn bricks_schema(&self) -> Result<Arc<BrdbSchema>, BrdbError> {
        if let Some(schema) = self.bricks_schema.read().unwrap().as_ref() {
            return Ok(schema.clone());
        }
        let schema = self
            .read_file("World/0/Bricks/ChunksShared.schema")?
            .as_slice()
            .read_brdb_schema()?;
        self.bricks_schema.write().unwrap().replace(schema.clone());
        Ok(schema)
    }
    /// Read the brick chunk indices for a specific grid
    pub fn brick_chunk_index(&self, grid_id: usize) -> Result<Vec<ChunkIndex>, BrdbError> {
        let brick_index = self
            .read_file(format!("World/0/Bricks/Grids/{grid_id}/ChunkIndex.mps"))?
            .as_slice()
            .read_brdb(&self.brick_chunk_index_schema()?, BRICK_CHUNK_INDEX_SOA)?;
        println!(
            "[debug] {}",
            brick_index.display(self.brick_chunk_index_schema()?.as_ref())
        );
        let chunk_indices = brick_index
            .prop("Chunk3DIndices")?
            .as_array()?
            .into_iter()
            .map(|s| {
                Ok(ChunkIndex {
                    x: s.prop("X")?.as_brdb_i16()?,
                    y: s.prop("Y")?.as_brdb_i16()?,
                    z: s.prop("Z")?.as_brdb_i16()?,
                })
            })
            .collect::<Result<Vec<_>, BrdbSchemaError>>()?;
        Ok(chunk_indices)
    }
    pub fn brick_chunk_soa(
        &self,
        grid_id: usize,
        chunk: ChunkIndex,
    ) -> Result<BrdbStruct, BrdbError> {
        let path = format!("World/0/Bricks/Grids/{grid_id}/Chunks/{chunk}.mps");
        let mps = self
            .read_file(path)?
            .as_slice()
            .read_brdb(&self.bricks_schema()?, BRICK_CHUNK_SOA)?;
        match mps {
            BrdbValue::Struct(s) => Ok(*s),
            ty => Err(BrdbError::Schema(BrdbSchemaError::ExpectedType(
                "Struct".to_string(),
                ty.get_type().to_owned(),
            ))),
        }
    }
    /// Read the shared entity chunk schema
    pub fn entities_schema(&self) -> Result<Arc<BrdbSchema>, BrdbError> {
        if let Some(schema) = self.entities_schema.read().unwrap().as_ref() {
            return Ok(schema.clone());
        }
        let schema = self
            .read_file("World/0/Entities/ChunksShared.schema")?
            .as_slice()
            .read_brdb_schema_with_data(self.global_data()?)?;
        self.entities_schema
            .write()
            .unwrap()
            .replace(schema.clone());
        Ok(schema)
    }
    pub fn entities_chunk_index_schema(&self) -> Result<Arc<BrdbSchema>, BrdbError> {
        if let Some(schema) = self.entity_chunk_index_schema.read().unwrap().as_ref() {
            return Ok(schema.clone());
        }
        let schema = self
            .read_file("World/0/Entities/ChunkIndex.schema")?
            .as_slice()
            .read_brdb_schema()?;
        self.entity_chunk_index_schema
            .write()
            .unwrap()
            .replace(schema.clone());
        Ok(schema)
    }

    /// Read the entity chunk indices
    pub fn entity_chunk_index(&self) -> Result<Vec<ChunkIndex>, BrdbError> {
        let entities_index = self
            .read_file("World/0/Entities/ChunkIndex.mps")?
            .as_slice()
            .read_brdb(&self.entities_chunk_index_schema()?, ENTITY_CHUNK_INDEX_SOA)?;
        let entity_chunks_ids = entities_index
            .prop("Chunk3DIndices")?
            .as_array()?
            .into_iter()
            .map(|s| {
                Ok(ChunkIndex {
                    x: s.prop("X")?.as_brdb_i16()?,
                    y: s.prop("Y")?.as_brdb_i16()?,
                    z: s.prop("Z")?.as_brdb_i16()?,
                })
            })
            .collect::<Result<Vec<_>, BrdbSchemaError>>()?;
        Ok(entity_chunks_ids)
    }

    pub fn entity_chunk(&self, chunk: ChunkIndex) -> Result<Vec<Entity>, BrdbError> {
        let global_data = self.global_data()?;
        let schema = self.entities_schema()?;
        let path = format!("World/0/Entities/Chunks/{chunk}.mps");
        let buf = self.read_file(path)?;
        let buf = &mut buf.as_slice();
        let illegal = "illegal".to_string();

        let mps = buf.read_brdb(&schema, BRICK_CHUNK_SOA)?;
        let soa = match mps {
            BrdbValue::Struct(s) => *s,
            ty => {
                return Err(BrdbError::Schema(BrdbSchemaError::ExpectedType(
                    "Struct".to_string(),
                    ty.get_type().to_owned(),
                )));
            }
        };

        let mut entity_data = Vec::new();
        let mut index = 0;
        let locked_flags = soa.prop("PhysicsLockedFlags")?;
        let sleeping_flags = soa.prop("PhysicsSleepingFlags")?;

        for counter in soa.prop("TypeCounters")?.as_array()? {
            let type_idx = counter.prop("TypeIndex")?.as_brdb_u32()?;
            let num_instances = counter.prop("NumEntities")?.as_brdb_u32()?;
            let type_name = global_data
                .entity_type_names
                .get_index(type_idx as usize)
                .unwrap_or(&illegal);

            let struct_name = lookup_entity_struct_name(type_name);
            for _ in 0..num_instances {
                let data: Arc<Box<dyn BrdbComponent>> = if let Some(struct_name) = struct_name {
                    let value = buf
                        .read_brdb(&schema, struct_name)
                        .map_err(|e| e.wrap(format!("Read entity {type_name}/{struct_name}")))?;
                    let component = LiteralComponent::new_from_data(
                        type_name,
                        struct_name,
                        None,
                        Arc::new(value.as_struct()?.as_hashmap()?),
                        [],
                    );
                    Arc::new(Box::new(component))
                } else {
                    Arc::new(Box::new(()))
                };

                entity_data.push(Entity {
                    asset: BString::from(type_name),
                    id: Some(
                        soa.prop("PersistentIndices")?
                            .index_unwrap(index)?
                            .as_brdb_u32()? as usize,
                    ),
                    owner_index: Some(
                        soa.prop("OwnerIndices")?
                            .index_unwrap(index)?
                            .as_brdb_u32()?,
                    ),
                    location: soa.prop("Locations")?.index_unwrap(index)?.try_into()?,
                    rotation: soa.prop("Rotations")?.index_unwrap(index)?.try_into()?,
                    velocity: soa
                        .prop("LinearVelocities")?
                        .index_unwrap(index)?
                        .try_into()?,
                    angular_velocity: soa
                        .prop("AngularVelocities")?
                        .index_unwrap(index)?
                        .try_into()?,
                    color_and_alpha: soa
                        .prop("ColorsAndAlphas")?
                        .index_unwrap(index)?
                        .try_into()?,

                    frozen: BitFlags::get_from_brdb_array(locked_flags, index)?,
                    sleeping: BitFlags::get_from_brdb_array(sleeping_flags, index)?,
                    data,
                });
                index += 1;
            }
        }
        Ok(entity_data)
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::brdb::{
        Brdb, assets,
        errors::BrdbError,
        schema::{ReadBrdbSchema, as_brdb::AsBrdbValue},
        tables::BrdbBlob,
        wrapper::{Brick, Entity, World, lookup_entity_struct_name, schemas::ENTITY_CHUNK_SOA},
    };

    /// This test will copy the sqlite schema to another file
    // #[test]
    // fn read_sqlite_schema() -> Result<(), Box<dyn std::error::Error>> {
    //     let mut path = PathBuf::from("./Parkour.brdb");
    //     if !path.exists() {
    //         return Ok(());
    //     }

    //     let db = Brdb::open(&path)?;
    //     path.set_extension("brdb.sql");
    //     std::fs::write(path, db.sqlite_schema()?.as_bytes())?;
    //     Ok(())
    // }

    #[test]
    fn test_memory_db() -> Result<(), Box<dyn std::error::Error>> {
        // Ensures the memory db can be created without errors
        let db = Brdb::new_memory()?;

        // Insert a blob, folder, and file
        let blob_id = db.insert_blob(vec![0], BrdbBlob::hash(&[0]), None)?;
        let folder_id = db.insert_folder("test_folder", None, 0)?;
        let file_id = db.insert_file("test", Some(folder_id), blob_id, 0)?;

        assert_eq!(
            db.get_fs()?.render(),
            "   |-- test_folder/\n   |   |-- test\n"
        );

        // Ensure the file can be read
        assert_eq!(db.read_file("test_folder/test")?, vec![0]);

        // Delete the file
        db.delete_file(file_id, 1)?;
        assert_eq!(db.get_fs()?.render(), "   |-- test_folder/\n");
        assert!(db.read_file("test_folder/test").is_err());

        // Delete the folder
        db.delete_folder(folder_id, 1)?;
        assert_eq!(db.get_fs()?.render(), "");

        // Ensure the blob can still be found
        assert!(db.find_blob(blob_id).is_ok());
        // Ensure the blob can be found by hash
        assert!(db.find_blob_by_hash(1, &BrdbBlob::hash(&[0])).is_ok());
        Ok(())
    }

    #[test]
    fn test_memory_save() -> Result<(), Box<dyn std::error::Error>> {
        // Ensures the memory db can be created without errors
        let db = Brdb::new_memory()?.into_reader();
        let mut world = World::new();
        world.bricks.push(Brick {
            position: (0, 0, 3).into(),
            color: (255, 0, 0).into(),
            ..Default::default()
        });
        db.save("test world", &world)?;

        let mps = db.brick_chunk_soa(1, (0, 0, 0).into())?;
        let color = mps.prop("ColorsAndAlphas")?.index(0)?.unwrap();
        assert_eq!(color.prop("R")?.as_brdb_u8()?, 255);
        assert_eq!(color.prop("G")?.as_brdb_u8()?, 0);
        assert_eq!(color.prop("B")?.as_brdb_u8()?, 0);
        assert_eq!(color.prop("A")?.as_brdb_u8()?, 5);

        Ok(())
    }

    /// Writes a world with one brick to test.brdb
    #[test]
    fn test_write_save() -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from("./test.brdb");

        // Ensures the memory db can be created without errors
        let db = Brdb::new(&path)?.into_reader();
        let mut world = World::new();
        world.meta.bundle.description = "Test World".to_string();
        world.bricks.push(Brick {
            position: (0, 0, 6).into(),
            color: (255, 0, 0).into(),
            ..Default::default()
        });
        db.save("test world", &world)?;

        println!("{}", db.get_fs()?.render());

        let soa = db.brick_chunk_soa(1, (0, 0, 0).into())?;
        let color = soa.prop("ColorsAndAlphas")?.index(0)?.unwrap();
        assert_eq!(color.prop("R")?.as_brdb_u8()?, 255);
        assert_eq!(color.prop("G")?.as_brdb_u8()?, 0);
        assert_eq!(color.prop("B")?.as_brdb_u8()?, 0);
        assert_eq!(color.prop("A")?.as_brdb_u8()?, 5);

        Ok(())
    }

    /// Writes a world with two bricks and a wire connection to wire_test.brdb
    #[test]
    fn test_write_wire_save() -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from("./wire_test.brdb");

        let db = if path.exists() {
            Brdb::open(path)?
        } else {
            Brdb::create(path)?
        };

        let mut world = World::new();
        world.meta.bundle.description = "Test World".to_string();

        let (a, a_id) = Brick {
            position: (0, 0, 1).into(),
            color: (255, 0, 0).into(),
            asset: assets::bricks::B_REROUTE,
            ..Default::default()
        }
        .with_component(assets::components::Rerouter)
        .with_id_split();
        let (b, b_id) = Brick {
            position: (15, 0, 1).into(),
            color: (255, 0, 0).into(),
            asset: assets::components::LogicGate::BoolNot.brick(),
            ..Default::default()
        }
        .with_component(assets::components::LogicGate::BoolNot.component())
        .with_id_split();

        world.add_bricks([a, b]);
        world.add_wire_connection(
            assets::components::LogicGate::BoolNot.output_of(b_id),
            assets::components::Rerouter::input_of(a_id),
        );

        db.save("test world", &world)?;

        println!("{}", db.get_fs()?.render());

        Ok(())
    }

    /// Writes a world with one brick to test.brdb
    #[test]
    fn test_write_entity_save() -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from("./entity_test.brdb");

        let db = if path.exists() {
            Brdb::open(path)?
        } else {
            Brdb::create(path)?
        };

        let mut world = World::new();
        world.meta.bundle.description = "Test World".to_string();
        world.add_brick_grid(
            Entity {
                frozen: true,
                location: (0.0, 0.0, 40.0).into(),
                ..Default::default()
            },
            [Brick {
                position: (0, 0, 3).into(),
                color: (0, 255, 0).into(),
                ..Default::default()
            }],
        );

        db.save("test world", &world)?;

        println!("{}", db.get_fs()?.render());

        Ok(())
    }

    /// Reads the world generated by `test_write_save` and prints the data.
    #[test]
    fn test_read_test() -> Result<(), BrdbError> {
        let path = PathBuf::from("./test.brdb");
        if !path.exists() {
            return Ok(());
        }
        let db = Brdb::open(path)?.into_reader();

        println!("{}", db.get_fs()?.render());

        let data = db.brick_chunk_soa(1, (0, 0, 0).into())?;
        println!("data: {data}");

        Ok(())
    }

    /// Read all the components and brick assets
    #[test]
    fn test_read_all_components() -> Result<(), BrdbError> {
        let path = PathBuf::from("./wires2components.brdb");
        if !path.exists() {
            return Ok(());
        }
        let db = Brdb::open(path)?.into_reader();

        println!("{}", db.get_fs()?.render());

        let data = db.global_data()?;
        println!("Basic Brick assets: {:?}", data.basic_brick_asset_names);
        println!("wire ports: {:?}", data.component_wire_port_names);
        println!("component types: {:?}", data.component_type_names);
        println!("component structs: {:?}", data.component_data_struct_names);

        let chunks = db.brick_chunk_index(1)?;
        println!("chunks: {chunks:?}");
        for chunk in chunks {
            let (soa, components) = db.component_chunk_soa(1, chunk)?;
            println!("components soa: {soa}");
            for c in components {
                println!("component: {c}");
            }
        }

        Ok(())
    }

    #[test]
    fn test() -> Result<(), BrdbError> {
        let path = PathBuf::from("./entity.brdb");
        if !path.exists() {
            return Ok(());
        }
        let db = Brdb::open(path)?.into_reader();

        let global_data = db.read_global_data()?;

        println!("{}", db.get_fs()?.render());

        println!(
            "Basic Brick assets: {:?}",
            global_data.basic_brick_asset_names
        );
        println!(
            "Proc Brick assets: {:?}",
            global_data.procedural_brick_asset_names
        );
        println!("Entity assets: {:?}", global_data.entity_type_names);

        let bricks = db.brick_chunk_soa(3, (-1, -1, -1).into())?;
        println!("Bricks: {bricks}");

        let entity_schema = db.entities_schema()?;

        for chunk in db.entity_chunk_index()? {
            let buf = db.read_file(format!("World/0/Entities/Chunks/{chunk}.mps"))?;
            let buf = &mut buf.as_slice();

            let entities = buf.read_brdb(&entity_schema, ENTITY_CHUNK_SOA)?;
            println!("entities: {}", entities.display(&entity_schema));

            let type_counters = entities.prop("TypeCounters")?.as_array()?;
            for counter in type_counters {
                let type_idx = counter.prop("TypeIndex")?.as_brdb_u32()?;
                let num_instances = counter.prop("NumEntities")?.as_brdb_u32()?;
                let type_name = global_data
                    .entity_type_names
                    .get_index(type_idx as usize)
                    .cloned()
                    .unwrap_or("illegal".to_string());
                let struct_name = lookup_entity_struct_name(&type_name)
                    .unwrap_or("unknown")
                    .to_string();

                println!(
                    "Component type {type_name}/{struct_name} (index {type_idx}) has {num_instances} instances"
                );

                if struct_name == "None" {
                    continue;
                }

                for _ in 0..num_instances {
                    let component = buf.read_brdb(&entity_schema, &struct_name)?;
                    println!("Component: {}", component.display(&entity_schema));
                }
            }
        }

        Ok(())
    }
}
