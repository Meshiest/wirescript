use rusqlite::blob::Blob;

pub struct BrdbBlob<'a> {
    pub blob_id: i64,
    pub compression: i64,
    pub size_uncompressed: i64,
    pub size_compressed: i64,
    pub delta_base_id: i64,
    pub hash: Blob<'a>,
    pub content: Blob<'a>,
}

#[derive(Default)]
pub struct BrdbRevision {
    pub revision_id: i64,
    pub description: String,
    pub created_at: i64,
}

#[derive(Default)]
pub struct BrdbFolder {
    pub folder_id: i64,
    pub parent_id: Option<i64>, // references folder_id
    pub name: String,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Default)]

pub struct BrdbFile {
    file_id: i64,
    parent_id: Option<i64>, // references folders(folder_id),
    name: String,
    content_id: Option<i64>,
    created_at: i64,
    deleted_at: Option<i64>,
}
