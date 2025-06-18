/// Describes an entire filesystem tree that needs to be written
/// Any `None` values indicate unchanged files or folders
/// Any absent entries will be deleted
/// All files will be hashed and checked for existing blobs
/// Any overwritten files will be marked as deleted
///
/// A revision will be created along with all of the pending
pub enum BrdbPendingFs {
    Root(Vec<BrdbPendingFs>),
    Folder(String, Option<Vec<BrdbPendingFs>>),
    File(String, Option<Vec<u8>>),
}
