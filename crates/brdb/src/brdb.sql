CREATE TABLE blobs (
				blob_id INTEGER PRIMARY KEY,
				compression INTEGER,
				size_uncompressed INTEGER,
				size_compressed INTEGER,
				delta_base_id INTEGER REFERENCES blobs(blob_id),
				hash BLOB,
				content BLOB
			);
CREATE TABLE revisions (
				revision_id INTEGER PRIMARY KEY,
				description TEXT,
				created_at INTEGER
			);
CREATE TABLE folders (
				folder_id INTEGER PRIMARY KEY,
				parent_id INTEGER REFERENCES folders(folder_id),
				name TEXT,
				created_at INTEGER,
				deleted_at INTEGER
			);
CREATE TABLE files (
				file_id INTEGER PRIMARY KEY,
				parent_id INTEGER REFERENCES folders(folder_id),
				name TEXT,
				content_id INTEGER REFERENCES blobs(blob_id),
				created_at INTEGER,
				deleted_at INTEGER
			);
CREATE INDEX blobs_size_hash ON blobs(size_uncompressed, hash);
CREATE INDEX folders_parent_name_deleted ON folders(parent_id, name, deleted_at);
CREATE INDEX files_parent_name_deleted ON files(parent_id, name, deleted_at);