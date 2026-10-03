use personal_ai_llm_openai::chatgpt::{Error, Registration, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub host_id: String,
    pub accounts: BTreeMap<String, Registration>,
}
pub struct Store {
    directory: PathBuf,
    _lock: File,
    pub data: Data,
}
pub fn check_label(label: &str) -> Result<()> {
    if label.is_empty()
        || label.len() > 64
        || !label
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(Error(
            "account label must contain 1..64 ASCII letters, digits, hyphens or underscores",
        ));
    }
    Ok(())
}
fn private(path: &Path, directory: bool) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(|_| Error("cannot inspect credential storage"))?;
    if m.file_type().is_symlink()
        || m.is_dir() != directory
        || (!directory && (!m.is_file() || m.nlink() != 1))
        || m.mode() & 0o077 != 0
    {
        return Err(Error(
            "credential storage must be a private directory (0700) with private regular files (0600), no links",
        ));
    }
    Ok(())
}
impl Store {
    pub fn open(directory: &Path) -> Result<Self> {
        // The parent must exist. Never loosen permissions or traverse a final symlink.
        match fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(_) => {
                return Err(Error(
                    "cannot create credential directory; create its parent first",
                ));
            }
        }
        private(directory, true)?;
        let lock_path = directory.join("lock");
        if lock_path.symlink_metadata().is_ok() {
            private(&lock_path, false)?;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|_| Error("cannot open credential lock"))?;
        private(&lock_path, false)?;
        lock.try_lock()
            .map_err(|_| Error("another chatgpt-connect command is active"))?;
        let path = directory.join("accounts.json");
        let data = match fs::symlink_metadata(&path) {
            Ok(_) => {
                private(&path, false)?;
                let mut bytes = Vec::new();
                File::open(&path)
                    .and_then(|f| f.take(1_048_577).read_to_end(&mut bytes))
                    .map_err(|_| Error("cannot read credentials"))?;
                if bytes.len() > 1_048_576 {
                    return Err(Error("credential file too large"));
                }
                serde_json::from_slice(&bytes).map_err(|_| Error("invalid credential file"))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data {
                host_id: Uuid::new_v4().urn().to_string(),
                accounts: BTreeMap::new(),
            },
            Err(_) => return Err(Error("cannot inspect credentials")),
        };
        let store = Self {
            directory: directory.into(),
            _lock: lock,
            data,
        };
        store.save()?;
        Ok(store)
    }
    pub fn save(&self) -> Result<()> {
        let bytes =
            serde_json::to_vec(&self.data).map_err(|_| Error("cannot encode credentials"))?;
        if bytes.len() > 1_048_576 {
            return Err(Error("credential file too large"));
        }
        let temp = self.directory.join(format!(".{}.tmp", Uuid::new_v4()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)
                .map_err(|_| Error("cannot create credential file"))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| Error("cannot persist credentials"))?;
            fs::rename(&temp, self.directory.join("accounts.json"))
                .map_err(|_| Error("cannot replace credentials"))?;
            File::open(&self.directory)
                .and_then(|f| f.sync_all())
                .map_err(|_| Error("cannot sync credential directory"))
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{Store, private};
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };
    use uuid::Uuid;
    #[test]
    fn persistence_locking_and_permissions() {
        let path = std::env::temp_dir().join(format!("chatgpt-store-{}", Uuid::new_v4()));
        let store = Store::open(&path).unwrap();
        let host = store.data.host_id.clone();
        assert!(Store::open(&path).is_err());
        private(&path.join("accounts.json"), false).unwrap();
        drop(store);
        assert_eq!(Store::open(&path).unwrap().data.host_id, host);
        fs::set_permissions(
            path.join("accounts.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(Store::open(&path).is_err());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn rejects_linked_and_public_storage() {
        let path = std::env::temp_dir().join(format!("chatgpt-links-{}", Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Store::open(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(path.join("target"), "do not modify").unwrap();
        symlink("target", path.join("lock")).unwrap();
        assert!(Store::open(&path).is_err());
        assert_eq!(
            fs::read_to_string(path.join("target")).unwrap(),
            "do not modify"
        );
        fs::remove_dir_all(path).unwrap();
    }
}
