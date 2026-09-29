use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{BlobHash, ParseHashError};

const COPY_CHUNK_SIZE: usize = 1024 * 1024;
const MAX_EXPORT_NAME_BYTES: usize = 211;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Destination(io::Error),
    NotFound(BlobHash),
    Corrupt {
        expected: BlobHash,
        actual: BlobHash,
    },
    InvalidHash(ParseHashError),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "store io: {e}"),
            StoreError::Destination(e) => write!(f, "destination io: {e}"),
            StoreError::NotFound(hash) => {
                write!(f, "blob {hash} not in store")
            }
            StoreError::Corrupt { expected, actual } => {
                write!(f, "blob {expected} is corrupt (hashes to {actual})")
            }
            StoreError::InvalidHash(e) => write!(f, "invalid blob hash: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl From<ParseHashError> for StoreError {
    fn from(e: ParseHashError) -> Self {
        StoreError::InvalidHash(e)
    }
}

struct TemporaryFile {
    path: Option<PathBuf>,
    file: File,
}

impl TemporaryFile {
    fn create(dir: &Path, prefix: &OsStr) -> io::Result<Self> {
        loop {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let mut name = OsString::from(prefix);
            name.push(format!("-{}-{id}.tmp", std::process::id()));
            let path = dir.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path: Some(path),
                        file,
                    })
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn commit(mut self, destination: &Path, parent: &Path) -> io::Result<()> {
        let path = self.path.as_ref().expect("temporary file is owned");
        fs::rename(path, destination)?;
        self.path.take();
        sync_directory(parent);
        Ok(())
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

#[cfg(unix)]
fn sync_directory(path: &Path) {
    let _ = File::open(path).and_then(|dir| dir.sync_all());
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) {}

enum CopyError {
    Read(io::Error),
    Write(io::Error),
}

impl From<CopyError> for StoreError {
    fn from(error: CopyError) -> Self {
        match error {
            CopyError::Read(e) | CopyError::Write(e) => StoreError::Io(e),
        }
    }
}

fn copy_hashed(
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<(BlobHash, u64), CopyError> {
    let mut buffer = vec![0; COPY_CHUNK_SIZE];
    let mut hasher = blake3::Hasher::new();
    let mut size = 0u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            result => result.map_err(CopyError::Read)?,
        };
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(CopyError::Write)?;
        hasher.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .ok_or_else(|| CopyError::Read(io::Error::other("blob size overflow")))?;
    }
    Ok((BlobHash::from_bytes(*hasher.finalize().as_bytes()), size))
}

fn open_blob(path: &Path, hash: BlobHash) -> Result<File, StoreError> {
    match File::open(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StoreError::NotFound(hash)),
        result => result.map_err(StoreError::Io),
    }
}

fn verify(path: &Path, expected: BlobHash) -> Result<(), StoreError> {
    let mut file = open_blob(path, expected)?;
    let (actual, _) = copy_hashed(&mut file, &mut io::sink())?;
    if actual != expected {
        return Err(StoreError::Corrupt { expected, actual });
    }
    Ok(())
}

pub struct BlobStore {
    root: PathBuf,
    _lock: File,
}

impl BlobStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("lock"))?;
        lock.try_lock().map_err(|e| match e {
            fs::TryLockError::WouldBlock => StoreError::Io(io::Error::new(
                io::ErrorKind::WouldBlock,
                "blob store is already open (lock held)",
            )),
            fs::TryLockError::Error(e) => StoreError::Io(e),
        })?;
        let tmp = root.join("tmp");
        fs::create_dir_all(&tmp)?;
        for entry in fs::read_dir(&tmp)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            let result = match entry.file_type() {
                Ok(kind) if kind.is_dir() => fs::remove_dir_all(entry.path()),
                Ok(_) => fs::remove_file(entry.path()),
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                if e.kind() != io::ErrorKind::NotFound {
                    return Err(e.into());
                }
            }
        }
        sync_directory(&tmp);
        Ok(Self { root, _lock: lock })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_of(&self, hash: BlobHash) -> PathBuf {
        self.root.join(hash.to_string())
    }

    fn stage(&self, mut reader: impl Read) -> Result<(TemporaryFile, BlobHash, u64), StoreError> {
        let mut temp = TemporaryFile::create(&self.root.join("tmp"), OsStr::new("tmp"))?;
        let (hash, size) = copy_hashed(&mut reader, &mut temp.file)?;
        Ok((temp, hash, size))
    }

    fn install(&self, temp: TemporaryFile, hash: BlobHash) -> Result<(), StoreError> {
        match verify(&self.path_of(hash), hash) {
            Ok(()) => Ok(()),
            Err(StoreError::Corrupt { .. } | StoreError::NotFound(_)) => {
                let destination = self.path_of(hash);
                temp.file.sync_all()?;
                temp.commit(&destination, parent_directory(&destination))?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn import_file(&self, path: impl AsRef<Path>) -> Result<BlobHash, StoreError> {
        self.import_reader(File::open(path)?)
    }

    pub fn import_reader(&self, reader: impl Read) -> Result<BlobHash, StoreError> {
        let (temp, hash, _) = self.stage(reader)?;
        self.install(temp, hash)?;
        Ok(hash)
    }

    pub fn write_verified(&self, expected: BlobHash, reader: impl Read) -> Result<u64, StoreError> {
        let (temp, actual, size) = self.stage(reader)?;
        if actual != expected {
            return Err(StoreError::Corrupt { expected, actual });
        }
        self.install(temp, expected)?;
        Ok(size)
    }

    pub fn export_file(
        &self,
        hash: BlobHash,
        dest_path: impl AsRef<Path>,
    ) -> Result<u64, StoreError> {
        let dest = dest_path.as_ref();
        let mut source = open_blob(&self.path_of(hash), hash)?;
        let parent = parent_directory(dest);
        let name = dest.file_name().ok_or_else(|| {
            StoreError::Destination(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination has no filename",
            ))
        })?;
        let name = name.to_string_lossy();
        let mut end = name.len().min(MAX_EXPORT_NAME_BYTES);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let prefix = format!(".{}.spirit", &name[..end]);
        let mut temp =
            TemporaryFile::create(parent, OsStr::new(&prefix)).map_err(StoreError::Destination)?;
        let (actual, size) = copy_hashed(&mut source, &mut temp.file).map_err(|e| match e {
            CopyError::Read(e) => StoreError::Io(e),
            CopyError::Write(e) => StoreError::Destination(e),
        })?;
        if actual != hash {
            return Err(StoreError::Corrupt {
                expected: hash,
                actual,
            });
        }
        temp.file.sync_all().map_err(StoreError::Destination)?;
        temp.commit(dest, parent).map_err(StoreError::Destination)?;
        Ok(size)
    }

    pub fn open_reader(&self, hash: BlobHash) -> Result<(u64, impl Read), StoreError> {
        let file = open_blob(&self.path_of(hash), hash)?;
        let size = file.metadata()?.len();
        Ok((size, file))
    }

    pub fn size(&self, hash: BlobHash) -> Result<u64, StoreError> {
        Ok(open_blob(&self.path_of(hash), hash)?.metadata()?.len())
    }

    pub fn put(&self, bytes: &[u8]) -> Result<BlobHash, StoreError> {
        self.import_reader(bytes)
    }

    pub fn get(&self, hash: BlobHash) -> Result<Vec<u8>, StoreError> {
        let bytes = match fs::read(self.path_of(hash)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound(hash))
            }
            Err(e) => return Err(e.into()),
        };
        let actual = BlobHash::of(&bytes);
        if actual != hash {
            return Err(StoreError::Corrupt {
                expected: hash,
                actual,
            });
        }
        Ok(bytes)
    }

    pub fn has(&self, hash: BlobHash) -> bool {
        self.path_of(hash).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    struct TestStore {
        store: BlobStore,
        _dir: TempDir,
    }

    impl TestStore {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            Self {
                store: BlobStore::open(dir.path()).unwrap(),
                _dir: dir,
            }
        }

        fn temp_count(&self) -> usize {
            fs::read_dir(self.store.root().join("tmp")).unwrap().count()
        }

        fn blob_count(&self) -> usize {
            fs::read_dir(self.store.root())
                .unwrap()
                .filter(|entry| {
                    let name = entry.as_ref().unwrap().file_name();
                    name.to_string_lossy().parse::<BlobHash>().is_ok()
                })
                .count()
        }
    }

    #[test]
    fn byte_api_remains_idempotent_and_verified() {
        let store = TestStore::new();
        let hash = store.store.put(b"riddles in the dark").unwrap();
        assert_eq!(store.store.put(b"riddles in the dark").unwrap(), hash);
        assert_eq!(store.store.get(hash).unwrap(), b"riddles in the dark");
        assert!(store.store.has(hash));
        assert_eq!(store.blob_count(), 1);
        fs::write(store.store.path_of(hash), b"tampered").unwrap();
        assert!(matches!(
            store.store.get(hash),
            Err(StoreError::Corrupt { .. })
        ));
        assert_eq!(store.store.put(b"riddles in the dark").unwrap(), hash);
        assert_eq!(store.store.get(hash).unwrap(), b"riddles in the dark");
    }

    #[test]
    fn import_and_export_multiple_chunks() {
        let store = TestStore::new();
        let data: Vec<u8> = (0..COPY_CHUNK_SIZE * 3 + 123)
            .map(|i| (i % 251) as u8)
            .collect();
        let input = store.store.root().join("input");
        let output = store.store.root().join("output");
        fs::write(&input, &data).unwrap();
        let hash = store.store.import_file(&input).unwrap();
        assert_eq!(store.store.import_file(&input).unwrap(), hash);
        assert_eq!(store.store.size(hash).unwrap(), data.len() as u64);
        let (size, mut reader) = store.store.open_reader(hash).unwrap();
        assert_eq!(size, data.len() as u64);
        let mut served = Vec::new();
        reader.read_to_end(&mut served).unwrap();
        assert_eq!(served, data);
        assert_eq!(store.store.export_file(hash, &output).unwrap(), size);
        assert_eq!(fs::read(&output).unwrap(), data);
        assert_eq!(store.temp_count(), 0);
        assert_eq!(store.blob_count(), 1);
    }

    #[test]
    fn concurrent_imports_use_independent_temporary_files() {
        let store = Arc::new(TestStore::new());
        let barrier = Arc::new(Barrier::new(8));
        let data = Arc::new(vec![42; COPY_CHUNK_SIZE * 2 + 19]);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (store, barrier, data) =
                    (Arc::clone(&store), Arc::clone(&barrier), Arc::clone(&data));
                std::thread::spawn(move || {
                    barrier.wait();
                    store.store.import_reader(data.as_slice()).unwrap()
                })
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), BlobHash::of(&data));
        }
        assert_eq!(store.blob_count(), 1);
        assert_eq!(store.temp_count(), 0);
        assert_eq!(store.store.get(BlobHash::of(&data)).unwrap(), *data);
    }

    #[test]
    fn corrupted_final_is_replaced_but_corrupt_export_preserves_destination() {
        let store = TestStore::new();
        let data = b"verified contents";
        let hash = BlobHash::of(data);
        fs::write(store.store.path_of(hash), b"corrupted").unwrap();
        let dest = store.store.root().join("destination");
        fs::write(&dest, b"leave this intact").unwrap();
        assert!(
            matches!(store.store.export_file(hash, &dest), Err(StoreError::Corrupt { expected, .. }) if expected == hash)
        );
        assert_eq!(fs::read(&dest).unwrap(), b"leave this intact");
        assert_eq!(fs::read_dir(store.store.root()).unwrap().count(), 4);
        assert_eq!(store.store.import_reader(&data[..]).unwrap(), hash);
        assert_eq!(
            store.store.export_file(hash, &dest).unwrap(),
            data.len() as u64
        );
        assert_eq!(fs::read(dest).unwrap(), data);
        assert_eq!(store.temp_count(), 0);
    }

    struct InterruptedReader(bool, bool);

    impl Read for InterruptedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                if self.1 {
                    panic!("cancelled while reading");
                }
                return Err(io::Error::other("input interrupted"));
            }
            self.0 = true;
            buffer[0] = 1;
            Ok(1)
        }
    }

    #[test]
    fn failed_or_cancelled_writes_remove_temporary_files() {
        let store = TestStore::new();
        let expected = BlobHash::of(b"expected complete file");
        assert!(
            matches!(store.store.write_verified(expected, &b"short"[..]), Err(StoreError::Corrupt { expected: e, .. }) if e == expected)
        );
        assert!(matches!(
            store
                .store
                .write_verified(expected, InterruptedReader(false, false)),
            Err(StoreError::Io(_))
        ));
        assert!(matches!(
            store.store.import_reader(InterruptedReader(false, false)),
            Err(StoreError::Io(_))
        ));
        let result = std::panic::catch_unwind(|| {
            let _ = store
                .store
                .write_verified(expected, InterruptedReader(false, true));
        });
        assert!(result.is_err());
        assert_eq!(store.temp_count(), 0);
        assert_eq!(store.blob_count(), 0);
        assert_eq!(
            store
                .store
                .write_verified(expected, &b"expected complete file"[..])
                .unwrap(),
            22
        );
        assert_eq!(
            store.store.get(expected).unwrap(),
            b"expected complete file"
        );
    }

    #[test]
    fn reopening_cleans_crash_leftovers_without_removing_blobs() {
        let dir = TempDir::new().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        let hash = store.put(b"completed").unwrap();
        fs::write(dir.path().join("tmp/abandoned"), b"partial").unwrap();
        fs::create_dir(dir.path().join("tmp/abandoned-directory")).unwrap();
        drop(store);
        let reopened = BlobStore::open(dir.path()).unwrap();
        assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
        assert_eq!(reopened.get(hash).unwrap(), b"completed");
    }

    #[test]
    fn second_open_fails_during_an_import_and_succeeds_after_drop() {
        struct PausedReader(Arc<Barrier>, bool);

        impl Read for PausedReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.1 {
                    return Ok(0);
                }
                self.1 = true;
                self.0.wait();
                self.0.wait();
                buffer[0] = 42;
                Ok(1)
            }
        }

        let dir = TempDir::new().unwrap();
        let store = Arc::new(BlobStore::open(dir.path()).unwrap());
        let barrier = Arc::new(Barrier::new(2));
        let worker_store = Arc::clone(&store);
        let worker_barrier = Arc::clone(&barrier);
        let worker = std::thread::spawn(move || {
            worker_store
                .import_reader(PausedReader(worker_barrier, false))
                .unwrap()
        });
        barrier.wait();
        assert!(
            matches!(BlobStore::open(dir.path()), Err(StoreError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock && e.to_string().contains("lock held"))
        );
        barrier.wait();
        let hash = worker.join().unwrap();
        assert_eq!(store.get(hash).unwrap(), [42]);
        drop(store);
        let reopened = BlobStore::open(dir.path()).unwrap();
        assert_eq!(reopened.get(hash).unwrap(), [42]);
    }

    #[cfg(unix)]
    #[test]
    fn export_replaces_symlink_with_new_file_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let store = TestStore::new();
        let hash = store.store.put(b"new").unwrap();
        let target = store.store.root().join("target");
        let reference = store.store.root().join("reference");
        let dest = store.store.root().join("link");
        fs::write(&target, b"old").unwrap();
        fs::write(&reference, b"").unwrap();
        symlink(&target, &dest).unwrap();
        store.store.export_file(hash, &dest).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert!(dest.symlink_metadata().unwrap().is_file());
        let mode = |path| fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode(&dest), mode(&reference));
    }

    #[test]
    fn export_long_multibyte_filename() {
        let store = TestStore::new();
        let hash = store.store.put(b"long name").unwrap();
        let dest = store
            .store
            .root()
            .join(format!("{}é{}", "x".repeat(210), "y".repeat(38)));
        store.store.export_file(hash, &dest).unwrap();
        assert_eq!(fs::read(dest).unwrap(), b"long name");
        assert_eq!(fs::read_dir(store.store.root()).unwrap().count(), 4);
    }

    #[cfg(unix)]
    #[test]
    fn export_succeeds_when_directory_cannot_be_opened() {
        use std::os::unix::fs::PermissionsExt;
        let store = TestStore::new();
        let hash = store.store.put(b"new contents").unwrap();
        let dir = store.store.root().join("write-only");
        fs::create_dir(&dir).unwrap();
        let permissions = fs::metadata(&dir).unwrap().permissions();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o300)).unwrap();
        if File::open(&dir).is_ok() {
            fs::set_permissions(&dir, permissions).unwrap();
            return;
        }
        let dest = dir.join("dest");
        let result = store.store.export_file(hash, &dest);
        fs::set_permissions(&dir, permissions).unwrap();
        assert_eq!(result.unwrap(), 12);
        assert_eq!(fs::read(dest).unwrap(), b"new contents");
    }

    #[test]
    fn export_into_directory_fails_without_a_leftover() {
        let store = TestStore::new();
        let hash = store.store.put(b"contents").unwrap();
        let dest = store.store.root().join("directory");
        fs::create_dir(&dest).unwrap();
        assert!(matches!(
            store.store.export_file(hash, &dest),
            Err(StoreError::Destination(_))
        ));
        assert!(dest.is_dir());
        assert_eq!(fs::read_dir(store.store.root()).unwrap().count(), 4);
    }

    #[test]
    fn export_source_failure_is_not_destination_failure() {
        let store = TestStore::new();
        let hash = BlobHash::of(b"directory");
        fs::create_dir(store.store.path_of(hash)).unwrap();
        let dest = store.store.root().join("out");
        assert!(matches!(
            store.store.export_file(hash, dest),
            Err(StoreError::Io(_))
        ));
        assert_eq!(fs::read_dir(store.store.root()).unwrap().count(), 3);
    }

    #[test]
    fn unwritable_export_directory_leaves_no_temporary_file() {
        let store = TestStore::new();
        let hash = store.store.put(b"contents").unwrap();
        let dir = store.store.root().join("unwritable");
        fs::create_dir(&dir).unwrap();
        let permissions = fs::metadata(&dir).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&dir, readonly).unwrap();
        let dest = dir.join("out");
        let result = store.store.export_file(hash, &dest);
        fs::set_permissions(&dir, permissions).unwrap();
        if let Err(error) = result {
            assert!(matches!(error, StoreError::Destination(_)));
            assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        }
    }

    #[test]
    fn streaming_operations_work_on_a_one_megabyte_stack() {
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(|| {
                let store = TestStore::new();
                let data = vec![7; COPY_CHUNK_SIZE * 2 + 17];
                let input = store.store.root().join("input");
                let output = store.store.root().join("output");
                fs::write(&input, &data).unwrap();
                let hash = store.store.import_file(&input).unwrap();
                assert_eq!(store.store.import_reader(data.as_slice()).unwrap(), hash);
                assert_eq!(
                    store.store.write_verified(hash, data.as_slice()).unwrap(),
                    data.len() as u64
                );
                assert_eq!(
                    store.store.export_file(hash, &output).unwrap(),
                    data.len() as u64
                );
                assert_eq!(fs::read(output).unwrap(), data);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn new_operations_report_not_found_and_invalid_hash() {
        let store = TestStore::new();
        let hash = BlobHash::of(b"absent");
        assert_eq!(parent_directory(Path::new("bare")), Path::new("."));
        assert!(!store.store.has(hash));
        assert!(matches!(store.store.get(hash), Err(StoreError::NotFound(h)) if h == hash));
        assert!(matches!(store.store.size(hash), Err(StoreError::NotFound(h)) if h == hash));
        assert!(matches!(store.store.open_reader(hash), Err(StoreError::NotFound(h)) if h == hash));
        assert!(
            matches!(store.store.export_file(hash, store.store.root().join("out")), Err(StoreError::NotFound(h)) if h == hash)
        );
        let error: StoreError = "invalid".parse::<BlobHash>().unwrap_err().into();
        assert!(matches!(error, StoreError::InvalidHash(_)));
    }
}
