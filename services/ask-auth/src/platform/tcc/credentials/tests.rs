use super::*;
struct TokenFile(PathBuf);
impl TokenFile {
    fn new(value: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!("ask-auth-dps-{}", uuid::Uuid::now_v7()));
        std::fs::write(&path, value).unwrap();
        Self(path)
    }
    fn source(&self) -> CredentialSource {
        CredentialSource::from_values(None, Some(self.0.clone()))
            .ok()
            .unwrap()
    }
}
impl Drop for TokenFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
#[tokio::test]
async fn credential_source_prefers_static_string_and_never_falls_back() {
    let file = TokenFile::new(b"file-token");
    let source = CredentialSource::from_values(Some("static-token".into()), Some(file.0.clone()))
        .ok()
        .unwrap();
    assert_eq!(source.load().await.ok().unwrap().bytes(), b"static-token");
    assert!(CredentialSource::from_values(Some("".into()), Some(file.0.clone())).is_err());
    assert!(CredentialSource::from_values(None, Some("relative/token".into())).is_err());
    assert!(CredentialSource::from_values(None, None).is_err());
}
#[tokio::test]
async fn files_are_fresh_bounded_and_reject_rotation_or_disappearance() {
    let file = TokenFile::new(b"initial-token");
    let source = file.source();
    let initial = source.load().await.ok().unwrap();
    assert!(source.check_current(&initial).await.is_ok());
    std::fs::write(&file.0, b"rotated-token").unwrap();
    assert!(source.check_current(&initial).await.is_err());
    assert_eq!(source.load().await.ok().unwrap().bytes(), b"rotated-token");
    std::fs::remove_file(&file.0).unwrap();
    assert!(source.check_current(&initial).await.is_err());
    assert!(source.load().await.is_err());
}
#[tokio::test]
async fn file_replacement_with_identical_contents_is_rejected() {
    let file = TokenFile::new(b"same-token");
    let source = file.source();
    let initial = source.load().await.ok().unwrap();
    let replacement = TokenFile::new(b"same-token");
    std::fs::rename(&replacement.0, &file.0).unwrap();
    assert!(source.check_current(&initial).await.is_err());
}
#[tokio::test]
async fn credential_file_bounds_and_nonregular_files_fail_closed() {
    for value in [
        vec![],
        vec![b'x'; MAX_TOKEN_BYTES + 1],
        b"token\n".to_vec(),
        vec![255],
    ] {
        assert!(TokenFile::new(&value).source().load().await.is_err());
    }
    assert!(TokenFile::new(&vec![b'x'; MAX_TOKEN_BYTES])
        .source()
        .load()
        .await
        .is_ok());
    let source = CredentialSource::from_values(None, Some(std::env::temp_dir()))
        .ok()
        .unwrap();
    assert!(source.load().await.is_err());
}
#[tokio::test]
async fn trusted_platform_symlinks_follow_rotation_and_fifo_never_blocks() {
    use std::os::unix::fs::symlink;
    let target = TokenFile::new(b"symlink-token");
    let link = TokenFile::new(b"unused");
    std::fs::remove_file(&link.0).unwrap();
    symlink(&target.0, &link.0).unwrap();
    assert_eq!(
        link.source().load().await.ok().unwrap().bytes(),
        b"symlink-token"
    );
    std::fs::remove_file(&link.0).unwrap();
    let outcome = std::process::Command::new("mkfifo")
        .arg(&link.0)
        .status()
        .unwrap();
    assert!(outcome.success());
    let outcome = tokio::time::timeout(Duration::from_millis(500), link.source().load()).await;
    assert!(outcome.is_ok_and(|result| result.is_err()));
}
