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
