//! Bounded share jobs; only the driver publishes files and records outcomes.
use super::*;
use crate::cli::high_level::OverwriteStrategy;
use crate::domain::share_access::ShareSession;
use futures::{stream::FuturesUnordered, StreamExt};
use std::sync::atomic::Ordering;
use tos_core::agent::request_id::capture_service_request_id;

type Prepared = (super::super::share_local::StagedFile, String);
struct Outcome {
    index: usize,
    result: Result<Prepared, CliError>,
    request_id: Option<String>,
    is_unknown: bool,
}
enum JobError {
    File(CliError),
    Tracking(CliError),
}
struct Driver<'a> {
    args: &'a ShareDownloadArgs,
    directory: &'a SafeDir,
    content: ContentClient,
}

pub(super) async fn transfer_files(
    global: &GlobalArgs,
    args: &ShareDownloadArgs,
    receiver: &mut ShareReceiver,
    directory: &SafeDir,
    progress: &mut Progress,
) -> Result<(), CliError> {
    let driver = Driver::open(global, args, directory, progress)?;
    let count = progress
        .journal
        .as_ref()
        .ok_or_else(|| invalid("missing manifest"))?
        .manifest
        .files
        .len();
    progress.states = (0..count).map(|_| Arc::new(AtomicU8::new(0))).collect();
    let receiver = tokio::sync::Mutex::new(receiver);
    let mut pending = FuturesUnordered::new();
    let (mut next, mut error, mut is_stopped) = (0, None, false);
    while next < count || !pending.is_empty() {
        while next < count && pending.len() < args.batch_concurrency.unwrap_or(16) && !is_stopped {
            let index = next;
            next += 1;
            match driver.prepare_skip(progress, index) {
                Ok(true) => continue,
                Err(failure) => {
                    track_error(failure, progress, &mut error, &mut is_stopped);
                    continue;
                }
                Ok(false) => {}
            }
            let entry = entry(progress, index)?.clone();
            progress.states[index].store(1, Ordering::SeqCst);
            // [Review Fix #ShareParity11] Keep nested HTTP transfer futures off the small worker/test stack.
            pending.push(Box::pin(run_job(
                &receiver,
                &driver.content,
                directory,
                entry,
                (index, progress.states[index].clone()),
            )));
        }
        let Some(outcome) = pending.next().await else {
            break;
        };
        if outcome.is_unknown {
            progress.is_issuing = true;
            is_stopped = true;
        }
        if let Err(failure) = driver.accept(progress, outcome) {
            track_error(failure, progress, &mut error, &mut is_stopped);
        }
    }
    error.map_or(Ok(()), Err)
}

impl<'a> Driver<'a> {
    fn open(
        global: &GlobalArgs,
        args: &'a ShareDownloadArgs,
        directory: &'a SafeDir,
        progress: &mut Progress,
    ) -> Result<Self, CliError> {
        let mut content = ContentClient::new()?
            .configured(progress.runtime.clone(), progress.display.range_display())
            .with_request_trace(global.request_trace.clone());
        if let Some(checkpoint) = &progress.checkpoint {
            content = content.with_checkpoint_directory(&PathBuf::from(format!(
                "{}.parts",
                checkpoint.path().display()
            )))?;
        }
        let paths = super::super::share_artifacts::Paths::resolve(global, args)?;
        let journal = progress
            .journal
            .as_ref()
            .ok_or_else(|| invalid("missing manifest"))?;
        let destination = PathBuf::from(&journal.identity.destination);
        paths.validate_against_manifest(
            &journal.manifest,
            &destination,
            progress.checkpoint.as_ref().map(Checkpoint::path),
        )?;
        // [Review Fix #ShareParity6] Retain paths before manifest I/O so failures can still publish/report them.
        progress.artifacts = Some(paths.open(args.is_report_failures_only)?);
        if let Some(artifacts) = &mut progress.artifacts {
            artifacts.write_manifest(&journal.manifest, &destination)?;
            for (path, failure) in &progress.directory_failures {
                artifacts.record(super::super::share_artifacts::Record {
                    source: &failure.remote,
                    destination: &destination.join(path),
                    status: "failed",
                    error: Some(&failure.message),
                    request_id: None,
                })?;
            }
        }
        Ok(Self {
            args,
            directory,
            content,
        })
    }

    fn prepare_skip(&self, progress: &mut Progress, index: usize) -> Result<bool, JobError> {
        if progress
            .journal
            .as_ref()
            .is_some_and(|journal| journal.completed[index].is_some())
        {
            record(progress, index, "skipped", None, None).map_err(JobError::Tracking)?;
            return Ok(true);
        }
        match entry(progress, index).and_then(|entry| should_skip(self.args, self.directory, entry))
        {
            Ok(false) => Ok(false),
            Ok(true) => {
                progress.display.file_finished();
                mark_skipped(progress, index).map_err(JobError::Tracking)?;
                Ok(true)
            }
            Err(error) => {
                progress.display.file_finished();
                Err(fail_file(progress, index, error, None))
            }
        }
    }

    fn accept(&self, progress: &mut Progress, outcome: Outcome) -> Result<(), JobError> {
        progress.display.file_finished();
        match outcome.result {
            Ok(prepared) => self.finish_file(progress, outcome.index, prepared, outcome.request_id),
            Err(error) => Err(fail_file(
                progress,
                outcome.index,
                error,
                outcome.request_id,
            )),
        }
    }

    fn finish_file(
        &self,
        progress: &mut Progress,
        index: usize,
        prepared: Prepared,
        request_id: Option<String>,
    ) -> Result<(), JobError> {
        let (staged, digest) = prepared;
        let remote = entry(progress, index).map_err(JobError::Tracking)?.clone();
        match should_skip(self.args, self.directory, &remote) {
            Ok(true) => return mark_skipped(progress, index).map_err(JobError::Tracking),
            Err(error) => return Err(fail_file(progress, index, error, None)),
            Ok(false) => {}
        }
        if let Err(error) =
            staged.publish(self.args.overwrite_strategy != Some(OverwriteStrategy::NoClobber))
        {
            return Err(fail_file(progress, index, error.into(), None));
        }
        let journal = progress
            .journal
            .as_mut()
            .ok_or_else(|| JobError::Tracking(invalid("missing manifest")))?;
        journal.completed[index] = Some(digest);
        progress.verified[index] = true;
        progress.states[index].store(6, Ordering::SeqCst);
        progress.request_ids.insert(index, request_id.clone());
        // [Review Fix #ShareParity8] Tracking failures retain already-published successes and stop new scheduling.
        let saved = save_completion_progress(progress, index);
        let reported = record(progress, index, "succeeded", None, request_id.as_deref());
        saved.and(reported).map_err(JobError::Tracking)?;
        if self
            .content
            .clear_completed_ranges(&remote.remote, &remote.local)
            .is_err()
        {
            progress.cleanup_warning = true;
        }
        Ok(())
    }
}

fn entry(progress: &Progress, index: usize) -> Result<&Entry, CliError> {
    progress
        .journal
        .as_ref()
        .and_then(|journal| journal.manifest.files.get(index))
        .ok_or_else(|| invalid("missing manifest entry"))
}

fn track_error(
    failure: JobError,
    progress: &mut Progress,
    first: &mut Option<CliError>,
    is_stopped: &mut bool,
) {
    match failure {
        JobError::File(error) => {
            if first.is_none() {
                *first = Some(error);
            }
        }
        JobError::Tracking(error) => {
            // Local artifact/checkpoint failures have no service request identity.
            progress.failure_status_code = None;
            progress.failure_request_id = None;
            progress.is_tracking_failure = true;
            *is_stopped = true;
            *first = Some(error);
        }
    }
}

fn should_skip(
    args: &ShareDownloadArgs,
    directory: &SafeDir,
    entry: &Entry,
) -> Result<bool, CliError> {
    let (parent, name) = directory.parent(&entry.local, true)?;
    if !parent.ordinary(&name)? {
        return Ok(false);
    }
    super::super::share_transfer_options::skip_existing(
        args,
        entry.remote.updated_at,
        &parent.read(&name)?.metadata()?,
    )
}

fn mark_skipped(progress: &mut Progress, index: usize) -> Result<(), CliError> {
    progress.skipped += 1;
    progress.skipped_files.insert(index);
    progress.states[index].store(7, Ordering::SeqCst);
    record(progress, index, "skipped", None, None)
}

fn fail_file(
    progress: &mut Progress,
    index: usize,
    error: CliError,
    request_id: Option<String>,
) -> JobError {
    let semantics = error.agent_semantics();
    if progress.failures.is_empty() && !progress.is_tracking_failure {
        progress.failure_status_code = semantics.status_code;
        progress.failure_request_id = request_id.clone();
    }
    progress.failures.insert(
        index,
        FileFailure {
            message: semantics.message.clone(),
            request_id: request_id.clone(),
            code: semantics.code,
        },
    );
    if let Some(state) = progress.states.get(index) {
        state.store(5, Ordering::SeqCst);
    }
    // [Review Fix #ShareParity9] Local failures get their own row and never borrow a previous response ID.
    match record(
        progress,
        index,
        "failed",
        Some(&semantics.message),
        request_id.as_deref(),
    ) {
        Ok(()) => JobError::File(error),
        Err(error) => JobError::Tracking(error),
    }
}

async fn run_job(
    receiver: &tokio::sync::Mutex<&mut ShareReceiver>,
    content: &ContentClient,
    directory: &SafeDir,
    entry: Entry,
    selected: (usize, Arc<AtomicU8>),
) -> Outcome {
    let (index, state) = selected;
    let (issued, issue_id) = capture_service_request_id(async {
        let session: ShareSession = receiver.lock().await.ensure_session().await?;
        state.store(2, Ordering::SeqCst);
        session
            .get_file_download_url(&entry.remote.path)
            .await
            .map_err(|error| map_issuance(error, &state))
    })
    .await;
    let issued = match issued {
        Ok(issued) => issued,
        Err(error) => {
            return Outcome {
                index,
                result: Err(error),
                request_id: issue_id,
                is_unknown: state.load(Ordering::SeqCst) == 2,
            }
        }
    };
    if super::super::share_content::validate_url(&issued.download_url).is_err() {
        return Outcome { index, result: Err(CliError::TransferFailed("[share_download_url_result_unknown] unusable download address; issuance was not replayed".into())), request_id: issue_id, is_unknown: true };
    }
    state.store(3, Ordering::SeqCst);
    let (result, request_id) = capture_service_request_id(Box::pin(content.download(
        &issued.download_url,
        &entry.remote,
        directory,
        &entry.local,
    )))
    .await;
    state.store(4, Ordering::SeqCst);
    Outcome {
        index,
        result,
        request_id,
        is_unknown: false,
    }
}

fn map_issuance(error: Error, state: &AtomicU8) -> CliError {
    let is_known = match &error {
        Error::Server(server) => server
            .status_code
            .is_some_and(|status| status < 500 && status != 408),
        Error::Client(_) | Error::Cli(_) | Error::UnifiedCredential(_) => true,
        _ => false,
    };
    if is_known {
        state.store(4, Ordering::SeqCst);
        map_ids_error(error)
    } else {
        CliError::TransferFailed("[share_download_url_result_unknown] download URL issuance failed; outcome may be unknown; not replayed".into())
    }
}

fn record(
    progress: &mut Progress,
    index: usize,
    status: &str,
    error: Option<&str>,
    request_id: Option<&str>,
) -> Result<(), CliError> {
    let journal = progress
        .journal
        .as_ref()
        .ok_or_else(|| invalid("missing manifest"))?;
    let selected = journal
        .manifest
        .files
        .get(index)
        .ok_or_else(|| invalid("missing manifest entry"))?;
    let destination = Path::new(&journal.identity.destination).join(&selected.local);
    if let Some(artifacts) = &mut progress.artifacts {
        artifacts.record(super::super::share_artifacts::Record {
            source: &selected.remote.path,
            destination: &destination,
            status,
            error,
            request_id,
        })?;
    }
    Ok(())
}

/// Publish valid report rows on success, failure and cancellation, retaining completed files.
pub(super) fn finalize(progress: &mut Progress, error: Option<&CliError>) -> Result<(), CliError> {
    let incomplete: Vec<_> = progress
        .states
        .iter()
        .enumerate()
        .filter_map(|(index, state)| {
            (1..=4)
                .contains(&state.load(Ordering::SeqCst))
                .then_some(index)
        })
        .collect();
    let message = error
        .map(|error| error.agent_semantics().message)
        .unwrap_or_else(|| "download interrupted; completed files retained".into());
    let mut failure = None;
    for index in incomplete {
        let row_error = CliError::TransferFailed(message.clone());
        if let JobError::Tracking(error) = fail_file(progress, index, row_error, None) {
            failure = Some(error);
        }
    }
    if let Some(artifacts) = &mut progress.artifacts {
        if let Err(error) = artifacts.finish() {
            failure = Some(error);
        }
    }
    failure.map_or(Ok(()), Err)
}
