use std::io::{Read, Write};

use crate::errors::BrdbFsError;

#[derive(Clone, Debug)]
pub struct BrdbBlob {
    pub blob_id: i64,
    pub compression: i64,
    pub size_uncompressed: i64,
    pub size_compressed: i64,
    pub delta_base_id: Option<i64>, // always null
    pub hash: Vec<u8>,
    pub content: Vec<u8>,
}

impl BrdbBlob {
    /// Get the BLAKE3 hash of the given content.
    pub fn hash(content: &[u8]) -> Vec<u8> {
        blake3::hash(content).as_bytes().to_vec()
    }

    /// Compress the given content using zstd with the specified level.
    pub fn compress(content: &[u8], zstd_level: i32) -> Result<Vec<u8>, std::io::Error> {
        let mut compressed = vec![];
        let mut enc = zstd::Encoder::new(&mut compressed, zstd_level)?;
        enc.write_all(content)?;
        enc.do_finish()?;
        Ok(compressed)
    }

    /// Read (and decompress) the content of a blob in the brdb filesystem.
    pub fn read(self) -> Result<Vec<u8>, BrdbFsError> {
        let content = if self.compression == 0 {
            self.content
        } else {
            // Ensure blob compressed content length is correct
            if self.content.len() != self.size_compressed as usize {
                return Err(BrdbFsError::InvalidSize {
                    name: "compressed content".to_string(),
                    found: self.content.len(),
                    expected: self.size_compressed as usize,
                });
            }

            // Decompress the content
            let mut output = vec![0u8; self.size_uncompressed as usize];
            zstd::Decoder::new(self.content.as_slice())
                .map_err(BrdbFsError::Decompress)?
                .read_exact(&mut output)
                .map_err(BrdbFsError::Decompress)?;
            output
        };

        // Verify the size of the decompressed content
        if content.len() != self.size_uncompressed as usize {
            return Err(BrdbFsError::InvalidSize {
                name: "uncompressed content".to_string(),
                found: content.len(),
                expected: self.size_uncompressed as usize,
            });
        }

        let hash = Self::hash(&content);

        // Verify the hash of the decompressed content
        if hash != self.hash.as_slice() {
            return Err(BrdbFsError::InvalidHash {
                found: hash,
                expected: self.hash,
            });
        }

        Ok(content)
    }
}

#[derive(Default, Clone, Debug)]
pub struct BrdbRevision {
    pub revision_id: i64,
    pub description: String,
    pub created_at: i64,
}

#[derive(Default, Clone, Debug)]
pub struct BrdbFolder {
    pub folder_id: i64,
    pub parent_id: Option<i64>, // references folder_id
    pub name: String,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Default, Clone, Debug)]

pub struct BrdbFile {
    pub file_id: i64,
    pub parent_id: Option<i64>, // references folders(folder_id),
    pub name: String,
    pub content_id: Option<i64>,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}
