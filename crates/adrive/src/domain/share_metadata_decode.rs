//! Share-only decoding keeps an absent size distinct from an explicitly empty file.
use super::types::{FileInfo, ListFilesOutput};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct ShareFileMetadata {
    #[serde(flatten)]
    info: FileInfo,
    #[serde(
        rename = "Size",
        alias = "size",
        alias = "FileSize",
        alias = "file_size",
        alias = "fileSize",
        alias = "ContentLength",
        alias = "content_length",
        alias = "contentLength"
    )]
    size: Option<i64>,
}
impl ShareFileMetadata {
    pub(super) fn into_info(mut self) -> FileInfo {
        // [Review Fix #D02] The legacy FileInfo default is zero; download plans must reject unknown size instead of claiming an exact zero-byte total.
        self.info.size = self.size.unwrap_or(-1);
        self.info
    }
}

#[derive(Deserialize)]
pub(super) struct ShareListMetadata {
    #[serde(flatten)]
    output: ListFilesOutput,
    #[serde(rename = "Files", alias = "files", default)]
    files: Vec<ShareFileMetadata>,
}
impl ShareListMetadata {
    pub(super) fn into_output(mut self) -> ListFilesOutput {
        self.output.files = self
            .files
            .into_iter()
            .map(ShareFileMetadata::into_info)
            .collect();
        self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn share_metadata_distinguishes_unknown_empty_and_aliased_size() {
        for field in [
            "Size",
            "size",
            "FileSize",
            "file_size",
            "fileSize",
            "ContentLength",
            "content_length",
            "contentLength",
        ] {
            for size in [0, 3] {
                let metadata: ShareFileMetadata =
                    serde_json::from_value(json!({"FilePath":"a",field:size})).unwrap();
                assert_eq!(metadata.into_info().size, size);
            }
        }
        let metadata: ShareFileMetadata = serde_json::from_value(json!({"FilePath":"a"})).unwrap();
        assert_eq!(metadata.into_info().size, -1);
        let list: ShareListMetadata = serde_json::from_value(json!({"files":[{"FilePath":"a"}],"Folders":[{"Folder":"empty/"}],"NextMarker":"next","IsTruncated":true})).unwrap();
        let output = list.into_output();
        assert_eq!(output.files[0].size, -1);
        assert_eq!(output.folders[0].folder, "empty/");
        assert_eq!(output.next_marker, "next");
        assert!(output.is_truncated);
    }
}
