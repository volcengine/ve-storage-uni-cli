//! Shared-file display and opt-in CSV artifacts retain the raw receiver metadata.
use super::common::output_result_with_columns;
use super::high_level::{
    adrive_ls_file_entry, adrive_ls_folder_entry, effective_explicit_manifest_path,
    normalize_ls_files_and_folders, parse_columns, sort_ls_files_and_folders, RollingCsvWriter,
    FILE_TABLE_COLUMNS,
};
use crate::cli::share::ShareFilesArgs;
use crate::domain::types::{FileInfo, ListFilesOutput};
use serde_json::{json, Value};
use tos_core::agent::{
    envelope::{Envelope, PaginationInfo},
    error::CliError,
    global_args::GlobalArgs,
    output::OutputFormat,
};

const MANIFEST_COLUMNS: &[&str] = &[
    "command",
    "operation",
    "source",
    "destination",
    "size",
    "etag",
    "crc64",
];

/// Render normalized display rows and optionally write a listing manifest.
/// Accepts authorized raw metadata; propagates invalid metadata, selector and filesystem errors.
pub(super) fn output_files(
    global: &GlobalArgs,
    args: &ShareFilesArgs,
    mut value: Value,
) -> Result<i32, CliError> {
    let mut listed: ListFilesOutput = serde_json::from_value(value.clone())?;
    if args.sort.is_some() {
        // [Review Fix #ShareFilesStructuredSort1] Explicit sorting applies to the bounded raw arrays in every format while preserving metadata types.
        sort_ls_files_and_folders(&mut listed.files, &mut listed.folders, args.sort.as_deref())?;
        value["Files"] = serde_json::to_value(&listed.files)?;
        value["Folders"] = serde_json::to_value(&listed.folders)?;
    }
    let manifest_path = effective_explicit_manifest_path(args.manifest_path.as_deref());
    write_manifest(manifest_path.as_deref(), &value, &listed)?;
    if let Some(path) = &manifest_path {
        value["manifest_path"] = json!(path);
    }
    let is_display_table = is_files_table(global);
    if is_display_table {
        value["entries"] = json!(display_entries(&listed, args)?);
    }
    let envelope =
        Envelope::success("ve-adrive share files", value).with_pagination(PaginationInfo {
            next_token: None,
            next_marker: (!listed.next_marker.is_empty()).then_some(listed.next_marker),
            total_returned: (listed.files.len() + listed.folders.len()) as u64,
        });
    output_result_with_columns(
        global,
        &envelope,
        is_display_table
            .then(|| parse_columns(args.columns.as_deref()).unwrap_or(FILE_TABLE_COLUMNS)),
    )?;
    Ok(0)
}

fn is_files_table(global: &GlobalArgs) -> bool {
    matches!(
        global.output.unwrap_or_else(OutputFormat::auto_detect),
        OutputFormat::Table | OutputFormat::Csv
    ) && !global
        .query
        .as_deref()
        .is_some_and(|query| !query.trim().is_empty())
}

fn display_entries(
    listed: &ListFilesOutput,
    args: &ShareFilesArgs,
) -> Result<Vec<Value>, CliError> {
    let (mut files, mut folders) = normalize_ls_files_and_folders(
        listed.files.clone(),
        listed.folders.clone(),
        args.path.as_deref().unwrap_or(""),
    );
    // [Review Fix #ShareFilesSort2] Numeric metadata must be sorted before human-readable size strings are generated.
    sort_ls_files_and_folders(&mut files, &mut folders, args.sort.as_deref())?;
    Ok(folders
        .iter()
        .map(|folder| adrive_ls_folder_entry(folder, args.human_readable))
        .chain(
            files
                .iter()
                .map(|file| adrive_ls_file_entry(file, args.human_readable)),
        )
        .collect())
}

fn write_manifest(
    path: Option<&str>,
    value: &Value,
    listed: &ListFilesOutput,
) -> Result<(), CliError> {
    let Some(path) = path else { return Ok(()) };
    let safe_url = value
        .get("ShareUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CliError::ValidationError("share listing is missing its safe source URL".into())
        })?;
    let mut writer = RollingCsvWriter::new(path, MANIFEST_COLUMNS)?;
    for file in &listed.files {
        writer.write_record(&file_manifest_record(safe_url, file))?;
    }
    for folder in &listed.folders {
        writer.write_record(&manifest_record(safe_url, &folder.folder, 0, "", ""))?;
    }
    writer.finish()
}

fn file_manifest_record(safe_url: &str, file: &FileInfo) -> Vec<String> {
    let crc64 = (file.hash_crc64_ecma != 0).then(|| file.hash_crc64_ecma.to_string());
    manifest_record(
        safe_url,
        &file.file_path,
        file.size.max(0) as u64,
        &file.etag,
        crc64.as_deref().unwrap_or(""),
    )
}

fn manifest_record(safe_url: &str, path: &str, size: u64, etag: &str, crc64: &str) -> Vec<String> {
    // [Review Fix #ShareFilesManifest1] Artifact sources retain one exact selection but never persist receiver passwords or tokens.
    let encoded_path = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("path", path)
        .finish();
    vec![
        "ve-adrive share files".into(),
        "list".into(),
        format!("{safe_url}?{encoded_path}"),
        String::new(),
        size.to_string(),
        etag.into(),
        crc64.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_source_encodes_one_exact_unicode_path_and_preserves_checksums() {
        let file: FileInfo = serde_json::from_value(json!({
            "FilePath":"docs/中文%?,\".txt","Size":-1,"ETag":"etag,\"value",
            "HashCRC64ECMA":u64::MAX,
        }))
        .unwrap();
        let record = file_manifest_record("https://example.test/share/s", &file);
        let source = url::Url::parse(&record[2]).unwrap();
        assert_eq!(
            source.query_pairs().collect::<Vec<_>>(),
            [("path".into(), file.file_path.into())]
        );
        assert_eq!(record[4], "0");
        assert_eq!(record[5], "etag,\"value");
        assert_eq!(record[6], u64::MAX.to_string());
    }

    #[test]
    fn table_display_sorts_and_formats_copies_without_mutating_metadata() {
        use clap::Parser;
        #[derive(Parser)]
        struct TestCli {
            #[command(flatten)]
            args: ShareFilesArgs,
        }
        let TestCli { args } = TestCli::try_parse_from([
            "files",
            "https://example.test/share/s",
            "-H",
            "--sort",
            "size",
        ])
        .unwrap();
        let listed: ListFilesOutput = serde_json::from_value(json!({
            "Files":[{"FilePath":"z","Size":2048},{"FilePath":"a","Size":12}],
            "Folders":[{"Folder":"sub/"}],
        }))
        .unwrap();
        let before = serde_json::to_value(&listed).unwrap();
        let entries = display_entries(&listed, &args).unwrap();
        assert_eq!(entries[0]["file_path"], "sub/");
        assert_eq!(entries[1]["file_path"], "a");
        assert_eq!(entries[2]["size"], "2.00 KiB");
        assert_eq!(serde_json::to_value(&listed).unwrap(), before);
    }
}
