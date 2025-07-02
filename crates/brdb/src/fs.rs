use std::{
    collections::{HashSet, VecDeque},
    fmt::Display,
};

use indexmap::IndexMap;

use crate::{
    Brdb,
    errors::BrdbFsError,
    pending::BrdbPendingFs,
    tables::{BrdbBlob, BrdbFile, BrdbFolder},
};

#[derive(Debug, Clone)]
pub enum BrdbFs {
    Root(IndexMap<String, BrdbFs>),
    Folder(BrdbFolder, IndexMap<String, BrdbFs>),
    File(BrdbFile),
}

pub(crate) fn now() -> i64 {
    // Use a high-resolution timer to get the current time in milliseconds
    let now = std::time::SystemTime::now();
    now.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

impl BrdbFs {
    pub fn write_pending(
        &self,
        description: &str,
        db: &Brdb,
        pending: BrdbPendingFs,
        zstd_level: Option<i32>,
    ) -> Result<(), BrdbFsError> {
        let created_at = now();
        let tx = db.conn.unchecked_transaction()?;
        // Create the revision
        db.create_revision(&description, created_at)
            .map_err(|e| e.wrap("Create Revision"))?;
        // Write the pending changes
        self.write_pending_internal(db, pending, created_at, zstd_level)?;
        // Commit the transaction (errors will result in rollback)
        tx.commit()?;
        Ok(())
    }

    fn write_pending_internal(
        &self,
        db: &Brdb,
        pending: BrdbPendingFs,
        created_at: i64,
        zstd_level: Option<i32>,
    ) -> Result<(), BrdbFsError> {
        let (parent, children, changes) = match (self, pending) {
            // Empty folder is noop
            (BrdbFs::Folder(_, _), BrdbPendingFs::Folder(None)) => return Ok(()),
            // Empty file is noop
            (BrdbFs::File(_), BrdbPendingFs::File(None)) => return Ok(()),
            // Directory handling
            (BrdbFs::Root(children), BrdbPendingFs::Root(files)) => (None, children, files),
            (BrdbFs::Folder(folder, children), BrdbPendingFs::Folder(Some(files))) => {
                (Some(folder.folder_id), children, files)
            }
            // Existing file handling
            (BrdbFs::File(file), BrdbPendingFs::File(Some(content))) => {
                let hash = BrdbBlob::hash(&content);

                // Check if this blob already exists
                if let Some(blob) = db.find_blob_by_hash(content.len(), &hash)? {
                    // File is unchanged
                    if file.content_id == Some(blob.blob_id) {
                        return Ok(());
                    }

                    // Delete the old file (because the content id changed)
                    db.delete_file(file.file_id, created_at)?;
                }

                // Insert the blob
                let content_id = db.insert_blob(content, hash, zstd_level)?;
                // Insert the file, reusing the old one's parent_id
                db.insert_file(&file.name, file.parent_id, content_id, created_at)?;
                return Ok(());
            }
            (l, r) => return Err(BrdbFsError::InvalidStructure(l.render(), r.to_string())),
        };

        let mut seen = HashSet::new();

        for (name, change) in changes {
            if seen.contains(&name) {
                return Err(BrdbFsError::DuplicateName(name.clone()));
            }
            seen.insert(name.clone());

            // If the child exists, update/replace it
            if let Some(child) = children.get(&name) {
                child
                    .write_pending_internal(db, change, created_at, zstd_level)
                    .map_err(|e| e.wrap(name))?;
                continue;
            }

            Self::insert_pending(db, &name, parent, change, created_at, zstd_level)
                .map_err(|e| e.wrap(name))?;
        }

        // Queue up all children that were not visited by the changes.
        let mut queue = children
            .iter()
            .filter_map(|(name, child)| (!seen.contains(name)).then_some(child))
            .collect::<VecDeque<_>>();

        // All descendants of non-visited children must be deleted.
        while let Some(child) = queue.pop_front() {
            match child {
                BrdbFs::Root(children) => {
                    for (_, child) in children {
                        queue.push_back(child);
                    }
                }
                BrdbFs::Folder(folder, children) => {
                    db.delete_folder(folder.folder_id, created_at)
                        .map_err(|e| e.wrap(format!("Delete Folder {}", folder.name)))?;
                    for (_, child) in children {
                        queue.push_back(child);
                    }
                }
                BrdbFs::File(file) => {
                    db.delete_file(file.file_id, created_at)
                        .map_err(|e| e.wrap(format!("Delete File {}", file.name)))?;
                }
            }
        }

        Ok(())
    }

    /// Insert a pending filesystem entry into the database without any
    /// existing structure.
    fn insert_pending(
        db: &Brdb,
        name: &str,
        parent: Option<i64>,
        pending: BrdbPendingFs,
        created_at: i64,
        zstd_level: Option<i32>,
    ) -> Result<(), BrdbFsError> {
        match pending {
            BrdbPendingFs::Root(files) => {
                return Err(BrdbFsError::InvalidStructure(
                    "root".to_string(),
                    BrdbPendingFs::Root(files).to_string(),
                ));
            }
            // Empty folder is a noop
            BrdbPendingFs::Folder(None) => {}
            // Emtpy file is a noop
            BrdbPendingFs::File(None) => {}
            BrdbPendingFs::Folder(Some(items)) => {
                // Create this folder, then insert its children
                let folder_id = db.insert_folder(&name, parent, now())?;
                for (name, child) in items {
                    // Recursively insert the child
                    Self::insert_pending(db, &name, Some(folder_id), child, created_at, zstd_level)
                        .map_err(|e| e.wrap(name))?;
                }
            }
            BrdbPendingFs::File(Some(content)) => {
                let hash = BrdbBlob::hash(&content);
                // Check if this blob already exists
                let content_id = if let Some(blob) = db.find_blob_by_hash(content.len(), &hash)? {
                    // If the blob already exists, reuse it
                    blob.blob_id
                } else {
                    // Insert the blob
                    db.insert_blob(content, hash, zstd_level)
                        .map_err(|e| e.wrap("Blob"))?
                };

                // Insert the file
                db.insert_file(&name, parent, content_id, created_at)?;
            }
        }
        Ok(())
    }

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
                    .map(|(name, child)| (name.to_owned(), child.to_pending()))
                    .collect(),
            ),
            BrdbFs::Folder(_folder, children) => BrdbPendingFs::Folder(Some(
                children
                    .iter()
                    .map(|(name, child)| (name.to_owned(), child.to_pending()))
                    .collect(),
            )),
            BrdbFs::File(_) => BrdbPendingFs::File(None),
        }
    }

    /// Navigate a brdb filesystem to a specific path.
    pub fn cd(&self, path: impl Display) -> Result<BrdbFs, BrdbFsError> {
        let path = path.to_string();
        if self.is_root() && path.starts_with("/") {
            return Err(BrdbFsError::AbsolutePathNotAllowed);
        }

        let is_last = !path.contains("/");

        // Recursively resolve the path
        match self {
            BrdbFs::Root(_) | BrdbFs::Folder(_, _) if is_last => Ok(self.clone()),
            BrdbFs::Root(children) | BrdbFs::Folder(_, children) => {
                // Unwrap safety - components.count() > 0
                let (first, _) = path.split_once("/").unwrap();
                if let Some(child) = children.get(first) {
                    child
                        .cd(path.strip_prefix(first).unwrap())
                        .map_err(|e| e.prepend(self.name()))
                } else {
                    Err(BrdbFsError::NotFound(format!("{}/{first}", self.name(),)))
                }
            }
            // Cannot cd in a file
            BrdbFs::File(_) if !is_last => Err(BrdbFsError::ExpectedDirectory(self.name())),
            BrdbFs::File(_) => Ok(self.clone()),
        }
    }

    /// Read the content of a file in the brdb filesystem.
    pub fn read_blob(&self, db: &Brdb) -> Result<BrdbBlob, BrdbFsError> {
        let BrdbFs::File(file) = self else {
            return Err(BrdbFsError::ExpectedFile(self.name().into()));
        };
        let Some(content_id) = file.content_id else {
            return Err(BrdbFsError::ExpectedFileContent(file.name.as_str().into()));
        };
        db.find_blob(content_id)
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

    pub fn render(&self) -> String {
        self.render_inner(0)
    }

    fn render_inner(&self, depth: usize) -> String {
        let pad = "   |".repeat(depth);
        match self {
            BrdbFs::Root(children) => {
                let mut output = String::new();
                for child in children.values() {
                    output.push_str(&child.render_inner(depth + 1));
                }
                output
            }
            BrdbFs::Folder(dir, children) => {
                let mut output = String::new();
                output.push_str(&format!("{pad}-- {}/\n", dir.name));
                for child in children.values() {
                    output.push_str(&child.render_inner(depth + 1));
                }
                output
            }
            BrdbFs::File(brdb_file) => {
                let file_path = if depth == 0 {
                    brdb_file.name.clone()
                } else {
                    format!("{pad}-- {}", brdb_file.name)
                };
                format!("{file_path}\n")
            }
        }
    }
}

impl BrdbFile {
    /// Read (and decompress) the content of a blob in the brdb filesystem.
    pub fn read(&self, db: &Brdb) -> Result<Vec<u8>, BrdbFsError> {
        let Some(content_id) = self.content_id else {
            return Err(BrdbFsError::ExpectedFileContent(self.name.as_str().into()).into());
        };
        db.find_blob(content_id)?.read()
    }
}
