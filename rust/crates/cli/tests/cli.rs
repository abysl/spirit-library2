use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Store(PathBuf);

impl Store {
    fn scratch(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("spirit-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn spirit2(&self, args: &[&str], stdin: &[u8]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_spirit"))
            .arg("--store")
            .arg(&self.0)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }

    fn ok(&self, args: &[&str], stdin: &[u8]) -> String {
        let output = self.spirit2(args, stdin);
        assert!(
            output.status.success(),
            "{args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn fails(&self, args: &[&str]) -> String {
        let output = self.spirit2(args, b"");
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        String::from_utf8(output.stderr).unwrap()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn is_hex_hash(line: &str) -> bool {
    line.len() == 64 && line.bytes().all(|b| b.is_ascii_hexdigit())
}

#[test]
fn put_a_file_then_get_it_back() {
    let store = Store::scratch("file");
    let file = store.0.with_extension("input");
    std::fs::write(&file, b"a blob of notes\n").unwrap();

    let hash = store.ok(&["blob", "put", file.to_str().unwrap()], b"");
    let hash = hash.trim();
    assert!(is_hex_hash(hash), "not a hash: {hash:?}");
    assert!(store.0.join(hash).is_file());

    assert_eq!(store.ok(&["blob", "get", hash], b""), "a blob of notes\n");
    let _ = std::fs::remove_file(file);
}

#[test]
fn put_reads_stdin_for_dash() {
    let store = Store::scratch("stdin");
    let hash = store.ok(&["blob", "put", "-"], b"from a pipe");
    assert_eq!(store.ok(&["blob", "get", hash.trim()], b""), "from a pipe");
}

#[test]
fn get_writes_a_file_with_out() {
    let store = Store::scratch("out");
    let hash = store.ok(&["blob", "put", "-"], b"twelve bytes");
    let copy = store.0.with_extension("copy");
    let told = store.ok(
        &["blob", "get", hash.trim(), "--out", copy.to_str().unwrap()],
        b"",
    );
    assert_eq!(told, format!("wrote 12 bytes to {}\n", copy.display()));
    assert_eq!(std::fs::read(&copy).unwrap(), b"twelve bytes");
    let _ = std::fs::remove_file(copy);
}

#[test]
fn put_is_idempotent() {
    let store = Store::scratch("idempotent");
    let first = store.ok(&["blob", "put", "-"], b"same bytes");
    let second = store.ok(&["blob", "put", "-"], b"same bytes");
    assert_eq!(first, second);
    assert_eq!(std::fs::read_dir(&store.0).unwrap().count(), 1);
}

#[test]
fn get_of_a_missing_blob_fails() {
    let store = Store::scratch("missing");
    let zero = "0".repeat(64);
    let stderr = store.fails(&["blob", "get", &zero]);
    assert_eq!(stderr, format!("spirit: blob {zero} not in store\n"));
}

#[test]
fn get_rejects_a_malformed_hash_before_touching_the_store() {
    let store = Store::scratch("malformed");
    let stderr = store.fails(&["blob", "get", "not-a-hash"]);
    assert!(stderr.contains("is not a blob hash"), "{stderr}");
    assert!(!store.0.exists());
}

#[test]
fn get_detects_a_corrupt_blob() {
    let store = Store::scratch("corrupt");
    let hash = store.ok(&["blob", "put", "-"], b"the real contents");
    let hash = hash.trim();
    std::fs::write(store.0.join(hash), b"tampered").unwrap();
    let stderr = store.fails(&["blob", "get", hash]);
    assert!(
        stderr.starts_with(&format!("spirit: blob {hash} is corrupt (hashes to ")),
        "{stderr}"
    );
}
