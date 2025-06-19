use std::{io::Read, path::Path};

use indexmap::IndexMap;
use rusqlite::params;

use crate::brdb::{
    Brdb,
    errors::BrdbFsError,
    revisions::BrdbPendingFs,
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

    /// Convert this filesystem to a pending filesystem with unchanged files.
    pub fn to_pending(&self) -> BrdbPendingFs {
        match self {
            BrdbFs::Root(children) => BrdbPendingFs::Root(
                children
                    .iter()
                    .map(|(_, child)| child.to_pending())
                    .collect(),
            ),
            BrdbFs::Folder(folder, children) => BrdbPendingFs::Folder(
                folder.name.clone(),
                Some(
                    children
                        .iter()
                        .map(|(_, child)| child.to_pending())
                        .collect(),
                ),
            ),
            BrdbFs::File(file) => BrdbPendingFs::File(file.name.clone(), None),
        }
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
        file.read_blob(db)
    }

    pub fn read(&self, db: &Brdb) -> Result<Vec<u8>, BrdbFsError> {
        let BrdbFs::File(file) = self else {
            return Err(BrdbFsError::ExpectedFile(self.name().into()));
        };
        file.read(db)
    }

    pub fn name(&self) -> String {
        match self {
            BrdbFs::Root(_) => "".to_string(),
            BrdbFs::Folder(folder, _) => folder.name.clone(),
            BrdbFs::File(file) => file.name.clone(),
        }
    }

    pub fn for_each(&self, func: &mut impl FnMut(&BrdbFs)) {
        func(self);
        match self {
            // Invoke for_each for each of the entries in each folder
            BrdbFs::Root(dir) | BrdbFs::Folder(_, dir) => {
                for fs in dir.values() {
                    fs.for_each(func)
                }
            }
            BrdbFs::File(_) => {}
        }
    }

    pub fn filter_map_file<T>(&self, mut func: impl FnMut(&BrdbFile) -> Option<T>) -> Vec<T> {
        let mut res = vec![];
        self.for_each(&mut |fs| match fs {
            BrdbFs::File(file) => {
                if let Some(r) = func(file) {
                    res.push(r);
                }
            }
            _ => {}
        });
        res
    }
}

impl BrdbFile {
    // TODO: write method, check hash and insert file. if a duplicate file exists, mark it as deleted
    // TODO: require the Brdb to have a list of pending transactions to commit during a revision

    /// Read the metadata for a file in the brdb filesystem.
    pub fn read_blob(&self, db: &Brdb) -> Result<BrdbBlob, BrdbFsError> {
        Ok(db
            .conn
            .query_one(
                "SELECT blob_id, compression, size_uncompressed, size_compressed, delta_base_id, hash, content
                FROM blobs
                WHERE blob_id = ?1;",
                params![self.content_id],
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
                })?)
    }

    /// Read (and decompress) the content of a blob in the brdb filesystem.
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
}
