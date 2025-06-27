use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    path::Path,
    sync::Arc,
};

use indexmap::{IndexMap, IndexSet};
use rusqlite::{Connection, params};

use crate::brdb::{
    errors::{BrdbError, BrdbFsError, BrdbSchemaError},
    fs::{BrdbFs, now},
    pending::BrdbPendingFs,
    schema::{BrdbSchemaGlobalData, BrdbStruct, BrdbValue, ReadBrdbSchema},
    tables::{BrdbBlob, BrdbFile, BrdbFolder},
    wrapper::schemas::{GLOBAL_DATA_SOA, OWNER_TABLE_SOA},
};

pub mod assets;
pub mod errors;
pub mod fs;
pub mod pending;
pub mod schema;
pub mod tables;
pub mod wrapper;

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

    /// Open an existing BRDB database at the specified path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BrdbError> {
        let db = Self {
            conn: Connection::open(path)?,
        };
        db.ensure_tables_exist()?;
        Ok(db)
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

    /// Ensure that all required tables exist in the database.
    fn ensure_tables_exist(&self) -> Result<(), BrdbError> {
        for t in &REQUIRED_TABLES {
            if !self.conn.table_exists(None, *t)? {
                return Err(BrdbError::MissingTable(t));
            }
        }
        Ok(())
    }

    /// Read the GlobalData from the BRDB database.
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
            entity_type_names: str_vec("EntityTypeNames")?,
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

    /// Read the Owners table from the BRDB database.
    pub fn read_owners(&self) -> Result<BrdbStruct, BrdbError> {
        let owners_schema = self
            .read_file("World/0/Owners.schema")?
            .as_slice()
            .read_brdb_schema()?;
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
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

    use crate::brdb::{
        Brdb, assets,
        errors::BrdbError,
        schema::{ReadBrdbSchema, as_brdb::AsBrdbValue},
        tables::BrdbBlob,
        wrapper::{
            Brick, World,
            schemas::{BRICK_CHUNK_SOA, BRICK_COMPONENT_SOA, BRICK_WIRE_SOA},
        },
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
        let db = Brdb::new_memory()?;
        let mut world = World::new();
        world.bricks.push(Brick {
            position: (0, 0, 3).into(),
            color: (255, 0, 0).into(),
            ..Default::default()
        });
        db.write_pending("test world", world.to_unsaved()?.to_pending()?)?;

        let global_data = db.read_global_data()?;
        let schema = db
            .read_file("World/0/Bricks/ChunksShared.schema")?
            .as_slice()
            .read_brdb_schema_with_data(global_data)?;
        let mps = db
            .read_file("World/0/Bricks/Grids/1/Chunks/0_0_0.mps")?
            .as_slice()
            .read_brdb(&schema, BRICK_CHUNK_SOA)?;
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
        let db = if path.exists() {
            Brdb::open(path)?
        } else {
            Brdb::create(path)?
        };
        let mut world = World::new();
        world.meta.bundle.description = "Test World".to_string();
        world.bricks.push(Brick {
            position: (0, 0, 6).into(),
            color: (255, 0, 0).into(),
            ..Default::default()
        });
        db.write_pending("test world", world.to_unsaved()?.to_pending()?)?;

        println!("{}", db.get_fs()?.render());

        let global_data = db.read_global_data()?;
        let schema = db
            .read_file("World/0/Bricks/ChunksShared.schema")?
            .as_slice()
            .read_brdb_schema_with_data(global_data)?;
        let mps = db
            .read_file("World/0/Bricks/Grids/1/Chunks/0_0_0.mps")?
            .as_slice()
            .read_brdb(&schema, BRICK_CHUNK_SOA)?;
        let color = mps.prop("ColorsAndAlphas")?.index(0)?.unwrap();
        assert_eq!(color.prop("R")?.as_brdb_u8()?, 255);
        assert_eq!(color.prop("G")?.as_brdb_u8()?, 0);
        assert_eq!(color.prop("B")?.as_brdb_u8()?, 0);
        assert_eq!(color.prop("A")?.as_brdb_u8()?, 5);

        Ok(())
    }

    /// Writes a world with one brick to test.brdb
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
            asset: assets::bricks::B_GATE_BOOL_NOT,
            ..Default::default()
        }
        .with_component(assets::components::LogicGate::BoolNot.component())
        .with_id_split();

        world.add_bricks([a, b]);
        world.add_wire_connection(
            assets::components::Rerouter::output_of(a_id),
            assets::components::LogicGate::BoolNot.input_of(b_id),
        );

        db.write_pending("test world", world.to_unsaved()?.to_pending()?)?;

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
        let db = Brdb::open(path)?;

        let global_data = db.read_global_data()?;

        println!("{}", db.get_fs()?.render());

        let schema = db
            .read_file("World/0/Bricks/ChunksShared.schema")?
            .as_slice()
            .read_brdb_schema_with_data(global_data.clone())?;

        let data = db.read_file("World/0/Bricks/Grids/1/Chunks/0_0_0.mps")?;
        let buf = &mut data.as_slice();
        let parsed = buf.read_brdb(&schema, BRICK_CHUNK_SOA)?;
        println!("data: {}", parsed.display(&schema));

        Ok(())
    }

    #[test]
    fn test() -> Result<(), BrdbError> {
        let path = PathBuf::from("./wire_test_a.brdb");
        if !path.exists() {
            return Ok(());
        }
        let db = Brdb::open(path)?;

        let global_data = db.read_global_data()?;

        // println!("{}", db.get_fs()?.render());

        let schema = db
            .read_file("World/0/Bricks/ComponentsShared.schema")?
            .as_slice()
            .read_brdb_schema_with_data(global_data.clone())?;

        let chunk_data = db.read_file("World/0/Bricks/Grids/1/Components/0_0_0.mps")?;
        let buf = &mut chunk_data.as_slice();
        let parsed = buf.read_brdb(&schema, BRICK_COMPONENT_SOA)?;
        println!("Components: {}", parsed.display(&schema));

        let type_counters = parsed.prop("ComponentTypeCounters")?.as_array()?;
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

            println!(
                "Component type {type_name}/{struct_name} (index {type_idx}) has {num_instances} instances"
            );

            if struct_name == "None" {
                continue;
            }

            for _ in 0..num_instances {
                let component = buf.read_brdb(&schema, &struct_name)?;
                println!("Component: {}", component.display(&schema));
            }
        }

        let brick_schema = db
            .read_file("World/0/Bricks/ChunksShared.schema")?
            .as_slice()
            .read_brdb_schema_with_data(global_data.clone())?;
        let brick_data = db
            .read_file("World/0/Bricks/Grids/1/Chunks/0_0_0.mps")?
            .as_slice()
            .read_brdb(&brick_schema, BRICK_CHUNK_SOA)?;
        println!("Bricks: {}", brick_data.display(&brick_schema));

        println!("Wire Ports: {:?}", global_data.component_wire_port_names);
        println!(
            "Basic Brick assets: {:?}",
            global_data.basic_brick_asset_names
        );
        println!(
            "Proc Brick assets: {:?}",
            global_data.procedural_brick_asset_names
        );

        let wires_schema = db
            .read_file("World/0/Bricks/WiresShared.schema")?
            .as_slice()
            .read_brdb_schema()?;
        let wires_data = db
            .read_file("World/0/Bricks/Grids/1/Wires/0_0_0.mps")?
            .as_slice()
            .read_brdb(&wires_schema, BRICK_WIRE_SOA)?;
        println!("Wires: {}", wires_data.display(&wires_schema));

        Ok(())
    }
}
