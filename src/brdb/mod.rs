use std::path::Path;

use rusqlite::{Connection, params};

use crate::brdb::errors::BrdbError;

pub mod errors;
pub mod tables;

pub struct Brdb {
    conn: Connection,
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

        Ok(Self { conn })
    }

    pub fn tree(&self, parent: Option<i64>, depth: usize) -> Result<(), BrdbError> {
        let dirs = if let Some(p) = parent {
            self.conn.prepare("SELECT name, folder_id FROM folders WHERE parent_id = ?1 AND deleted_at IS NULL ORDER BY name;")?
                .query_map(params![p], |row| Ok((row.get(0)?, row.get(1)?)))?
                .filter_map(|res: Result<(String, i64), _>| res.ok())
                .collect::<Vec<_>>()
        } else {
            self.conn.prepare("SELECT name, folder_id FROM folders WHERE parent_id IS NULL AND deleted_at IS NULL ORDER BY name;")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .filter_map(|res: Result<(String, i64), _>| res.ok())
                .collect::<Vec<_>>()
        };

        let pad = " | ".repeat(depth);
        for (name, id) in dirs {
            println!("{pad}{name}/");
            self.tree(Some(id), depth + 1)?;
        }

        if parent.is_some() {
            self.conn.prepare("SELECT name, file_id FROM files WHERE parent_id = ?1 AND deleted_at IS NULL ORDER BY name;")?
                    .query_map(params![parent], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .for_each(|res: Result<(String, i64), _>| {
                        let Ok((name, id)) = res else {
                            return;
                        };

                        println!("{pad}{name}");
                    });
        }

        Ok(())
    }
}

#[test]
fn test() {
    let db = Brdb::open("./Parkour.brdb").expect("foo");
    db.tree(None, 0).unwrap();
}
