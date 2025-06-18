use std::{io::Read, path::Path};

use indexmap::IndexMap;
use rusqlite::params;

use crate::brdb::{
    Brdb,
    errors::BrdbFsError,
    tables::{BrdbBlob, BrdbFile, BrdbFolder},
};

#[derive(Debug, Clone)]
pub enum BrdbFs {
    Root(IndexMap<String, BrdbFs>),
    Folder(BrdbFolder, IndexMap<String, BrdbFs>),
    File(BrdbFile),
}

impl BrdbFs {
    pub fn is_root(&self) -> bool {
        matches!(self, BrdbFs::Root(_))
    }

    pub fn is_folder(&self) -> bool {
        matches!(self, BrdbFs::Folder(_, _))
    }

    pub fn is_file(&self) -> bool {
        matches!(self, BrdbFs::File(_))
    }

    /// Navigate a brdb filesystem to a specific path.
    pub fn cd(&self, path: impl AsRef<Path>) -> Result<BrdbFs, BrdbFsError> {
        let path = path.as_ref();
        if self.is_root() && path.has_root() {
            return Err(BrdbFsError::AbsolutePathNotAllowed);
        }

        // Recursively resolve the path
        match self {
            BrdbFs::Root(_) if path.components().count() == 0 => Ok(self.clone()),
            BrdbFs::Root(children) => {
                // Unwrap safety - components.count() > 0
                let first = path.components().next().unwrap();
                if let Some(child) = children.get(first.as_os_str().to_str().unwrap()) {
                    child
                        .cd(path.strip_prefix(first).unwrap())
                        .map_err(|e| e.prepend(self.name()))
                } else {
                    Err(BrdbFsError::NotFound(self.name().into()))
                }
            }
            BrdbFs::Folder(_, children) => {
                if path.components().count() == 0 {
                    return Ok(self.clone());
                }
                let first = path.components().next().unwrap();
                if let Some(child) = children.get(first.as_os_str().to_str().unwrap()) {
                    child
                        .cd(path.strip_prefix(first).unwrap())
                        .map_err(|e| e.prepend(self.name()))
                } else {
                    Err(BrdbFsError::NotFound(self.name().into()))
                }
            }
            // Cannot cd in a file
            BrdbFs::File(_) if path.components().count() > 0 => {
                Err(BrdbFsError::ExpectedDirectory(self.name().into()))
            }
            BrdbFs::File(_) => Ok(self.clone()),
        }
    }

    /// Read the content of a file in the brdb filesystem.
    pub fn read_blob(&self, db: &Brdb) -> Result<BrdbBlob, BrdbFsError> {
        let BrdbFs::File(file) = self else {
            return Err(BrdbFsError::ExpectedFile(self.name().into()));
        };

        let blob = db
            .conn
            .query_one(
                "SELECT blob_id, compression, size_uncompressed, size_compressed, delta_base_id, hash, content
                FROM blobs
                WHERE blob_id = ?1;",
                params![file.content_id],
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

        // TODO: decompress

        Ok(blob)
    }

    pub fn read(&self, db: &Brdb) -> Result<Vec<u8>, BrdbFsError> {
        let blob = self.read_blob(db)?;

        let content = if blob.compression == 0 {
            blob.content
        } else {
            // Ensure blob compressed content length is correct
            if blob.content.len() != blob.size_compressed as usize {
                return Err(BrdbFsError::InvalidSize {
                    name: "compressed content".to_string(),
                    found: blob.content.len(),
                    expected: blob.size_compressed as usize,
                });
            }

            // Decompress the content
            let mut output = vec![0u8; blob.size_uncompressed as usize];
            zstd::Decoder::new(&blob.content[..])
                .map_err(BrdbFsError::Decompress)?
                .read_exact(&mut output)?;
            output
        };

        // Verify the size of the decompressed content
        if content.len() != blob.size_uncompressed as usize {
            return Err(BrdbFsError::InvalidSize {
                name: "uncompressed content".to_string(),
                found: content.len(),
                expected: blob.size_uncompressed as usize,
            });
        }

        let hash = blake3::hash(&content);
        let hash = hash.as_bytes();

        // Verify the hash of the decompressed content
        if hash != blob.hash.as_slice() {
            return Err(BrdbFsError::InvalidHash {
                found: hash.to_vec(),
                expected: blob.hash,
            });
        }

        Ok(content)
    }

    // TODO: write method, check hash and insert file. if a duplicate file exists, mark it as deleted
    // TODO: require the Brdb to have a list of pending transactions to commit during a revision

    fn name(&self) -> String {
        match self {
            BrdbFs::Root(_) => "".to_string(),
            BrdbFs::Folder(folder, _) => folder.name.clone(),
            BrdbFs::File(file) => file.name.clone(),
        }
    }
}
