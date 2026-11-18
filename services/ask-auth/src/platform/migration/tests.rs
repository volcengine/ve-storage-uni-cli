use super::*;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

struct Directory(std::path::PathBuf);
impl Directory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("ask-auth-migration-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, contents: &[u8], mode: u32) -> std::path::PathBuf {
        use std::io::Write;
        let path = self.0.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&path)
            .unwrap();
        file.write_all(contents).unwrap();
        path
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn configuration(ca: &Path) -> String {
    format!(
        r#"{{"schema_version":1,"database_url":"mysql://synthetic:synthetic@127.0.0.1/isolated","database_ca_path":"{}","maximum_connections":1}}"#,
        ca.display()
    )
}
#[test]
#[cfg(unix)]
fn protected_operator_file_accepts_only_owner_readable_regular_json() {
    let directory = Directory::new();
    let ca = directory.write("ca.pem", b"synthetic CA", 0o600);
    let path = directory.write("operator.json", configuration(&ca).as_bytes(), 0o600);
    assert!(read_config(&path).is_ok());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(read_config(&path).is_ok());
    for mode in [0o644, 0o640, 0o660, 0o700, 0o200] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(read_config(&path).is_err());
    }
}
#[test]
#[cfg(unix)]
fn symlink_fifo_directory_and_oversized_inputs_fail_without_hanging() {
    let directory = Directory::new();
    let ca = directory.write("ca.pem", b"synthetic CA", 0o600);
    let path = directory.write("operator.json", configuration(&ca).as_bytes(), 0o600);
    let link = directory.0.join("symlink.json");
    std::os::unix::fs::symlink(path, &link).unwrap();
    assert!(read_config(&link).is_err());
    assert!(read_config(&directory.0).is_err());
    let fifo = directory.0.join("fifo");
    let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let started = std::time::Instant::now();
    assert!(read_config(&fifo).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    let huge = directory.write("huge.json", &vec![b' '; 32769], 0o600);
    assert!(read_config(&huge).is_err());
}
#[test]
#[cfg(unix)]
fn malformed_migration_config_never_retains_secret_or_path() {
    let directory = Directory::new();
    let ca = directory.write("ca.pem", b"synthetic CA", 0o600);
    for (index, text) in [
        "[1,\"SECRET_DSN\"]".into(),
        configuration(&ca).replace("\"maximum_connections\":1", "\"maximum_connections\":2"),
        configuration(&ca).replace(
            "\"schema_version\":1",
            "\"schema_version\":1,\"schema_version\":1",
        ),
        configuration(&ca).replace(
            "mysql://synthetic:synthetic@127.0.0.1/isolated",
            "SECRET_DSN",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let path = directory.write(&format!("bad-{index}.json"), text.as_bytes(), 0o600);
        let error = read_config(&path).err().unwrap();
        assert!(!error.to_string().contains("SECRET"));
        assert!(!format!("{error:?}").contains("SECRET"));
    }
    assert!(read_config(Path::new("relative.json")).is_err());
}
