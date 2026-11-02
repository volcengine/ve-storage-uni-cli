//! Share receiver adaptation of the existing cp terminal progress styles.
use super::high_level::{batch_progress, PlanScanProgress};
use crate::cli::high_level::ProgressGranularity;
use crate::cli::share::ShareDownloadArgs;
use indicatif::ProgressBar;
use std::{
    io::{IsTerminal, Write},
    time::Duration,
};
use tos_core::agent::global_args::GlobalArgs;

#[derive(Default)]
pub(super) struct DownloadProgress {
    is_enabled: bool,
    is_terminal: bool,
    is_scan_enabled: bool,
    scan: Option<PlanScanProgress>,
    transfer: Option<ProgressBar>,
    processed: usize,
    total: usize,
    granularity: Option<ProgressGranularity>,
}

impl DownloadProgress {
    /// Select terminal/forced progress without reading anonymous receiver account files.
    pub(super) fn new(global: &GlobalArgs, args: &ShareDownloadArgs) -> Self {
        let is_terminal = std::io::stderr().is_terminal();
        // [Review Fix #ShareProgress1] Anonymous receipt must not load account configuration just to decide whether to draw progress.
        let is_enabled = !global.quiet && !args.is_no_progress && (args.is_progress || is_terminal);
        Self {
            is_enabled,
            is_terminal,
            is_scan_enabled: super::high_level::effective_list_echo_enabled(
                global,
                args.is_list_echo,
                args.is_no_list_echo,
            ),
            scan: None,
            transfer: None,
            processed: 0,
            total: 0,
            granularity: args.progress_granularity,
        }
    }

    /// Show scanning after password interaction has completed; never display the share URL.
    pub(super) fn begin_scan(&mut self) {
        self.scan = Some(PlanScanProgress::new(
            self.is_scan_enabled && self.is_terminal,
            "ve-adrive share download plan",
            "shared files",
        ));
        self.print_scan_line("ve-adrive share download plan: scanning shared files");
    }

    /// Replace scanning with file-count progress, initially crediting validated resume files.
    pub(super) fn begin_transfer(&mut self, total: usize, skipped: usize) {
        self.finish_scan(total);
        self.total = total;
        self.processed = skipped;
        self.transfer = batch_progress(
            "ve-adrive share download",
            total as u64,
            self.is_enabled && self.is_terminal,
        );
        if let Some(bar) = &self.transfer {
            bar.set_position(skipped as u64);
            bar.enable_steady_tick(Duration::from_millis(200));
        }
        self.print_plain_progress();
    }

    /// Finish metadata scanning independently of content-progress flags, as cp does.
    pub(super) fn finish_scan(&mut self, total: usize) {
        if let Some(mut scan) = self.scan.take() {
            scan.finish_with_count(total as u64, "file(s)");
        }
        self.print_scan_line(&format!(
            "ve-adrive share download plan: {total} file(s) discovered"
        ));
    }

    fn print_scan_line(&mut self, line: &str) {
        // [Review Fix #ShareParity4] Explicit list echo works on pipes while quiet/no-list-echo remain authoritative.
        if self.is_scan_enabled
            && !self.is_terminal
            && writeln!(std::io::stderr().lock(), "{line}").is_err()
        {
            self.is_scan_enabled = false;
        }
    }

    /// Advance one processed file, including a definite failure, as cp batch progress does.
    pub(super) fn file_finished(&mut self) {
        self.processed += 1;
        if let Some(bar) = &self.transfer {
            bar.inc(1);
        }
        self.print_plain_progress();
    }

    fn print_plain_progress(&mut self) {
        // [Review Fix #ShareProgress2] Explicit progress on a pipe uses readable lines; indicatif hides terminal draw targets when stderr is not a TTY.
        let line = format!(
            "ve-adrive share download: {}/{} file(s) processed",
            self.processed, self.total
        );
        self.print_plain_line(&line);
    }

    fn print_plain_line(&mut self, line: &str) {
        if self.is_enabled && !self.is_terminal {
            // [Review Fix #ShareProgress4] Closing a progress pipe must not panic or interrupt a transfer/checkpoint update.
            if writeln!(std::io::stderr().lock(), "{line}").is_err() {
                self.is_enabled = false;
            }
        }
    }

    /// Stop indicators before the final envelope, also safe to call during cancellation cleanup.
    pub(super) fn finish(&mut self) {
        self.scan.take();
        if let Some(bar) = self.transfer.take() {
            bar.finish_and_clear();
        }
    }
}

impl Drop for DownloadProgress {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Clone)]
pub(super) struct RangeDisplay {
    is_enabled: bool,
    is_terminal: bool,
    transfer: Option<ProgressBar>,
    granularity: ProgressGranularity,
}
impl Default for RangeDisplay {
    fn default() -> Self {
        Self {
            is_enabled: false,
            is_terminal: false,
            transfer: None,
            granularity: ProgressGranularity::Part,
        }
    }
}
impl DownloadProgress {
    /// Share the file indicator with range jobs; no URL or remote filename enters progress output.
    pub(super) fn range_display(&self) -> RangeDisplay {
        RangeDisplay {
            is_enabled: self.is_enabled,
            is_terminal: self.is_terminal,
            transfer: self.transfer.clone(),
            granularity: self.granularity.unwrap_or(ProgressGranularity::Part),
        }
    }
}
impl RangeDisplay {
    /// Begin per-file range completion feedback while retaining the aggregate file position.
    pub(super) fn begin(&self, total: u64, total_bytes: u64) -> RangeProgress {
        let progress = RangeProgress {
            display: self.clone(),
            completed: std::cell::Cell::new(0),
            is_output_closed: std::cell::Cell::new(false),
            total,
            total_bytes,
            completed_bytes: std::cell::Cell::new(0),
        };
        progress.show();
        progress
    }
}
pub(super) struct RangeProgress {
    display: RangeDisplay,
    completed: std::cell::Cell<u64>,
    is_output_closed: std::cell::Cell<bool>,
    total: u64,
    total_bytes: u64,
    completed_bytes: std::cell::Cell<u64>,
}
impl RangeProgress {
    /// Credit a verified range once; retries never advance the indicator twice.
    pub(super) fn completed(&self, bytes: u64) {
        self.completed.set(self.completed.get() + 1);
        self.completed_bytes
            .set(self.completed_bytes.get().saturating_add(bytes));
        self.show();
    }
    fn label(&self) -> String {
        let (unit, completed, total) = match self.display.granularity {
            ProgressGranularity::Part => ("ranges", self.completed.get(), self.total),
            ProgressGranularity::Byte => ("bytes", self.completed_bytes.get(), self.total_bytes),
        };
        format!("ve-adrive share download [{unit} {completed}/{total}]")
    }
    fn show(&self) {
        if !self.display.is_enabled || self.is_output_closed.get() {
            return;
        }
        let label = self.label();
        if self.display.is_terminal {
            if let Some(bar) = &self.display.transfer {
                bar.set_prefix(label);
            }
        } else if let Err(_error) = writeln!(std::io::stderr().lock(), "{label}") {
            // A closed progress pipe never changes the transfer outcome.
            self.is_output_closed.set(true);
        }
    }
}
impl Drop for RangeProgress {
    fn drop(&mut self) {
        if let Some(bar) = &self.display.transfer {
            bar.set_prefix("ve-adrive share download");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::high_level::ProgressGranularity;
    #[test]
    fn range_progress_credits_parts_or_verified_bytes_without_retry_duplicates() {
        let display = RangeDisplay::default();
        let progress = display.begin(3, 20);
        progress.completed(8);
        assert_eq!(progress.label(), "ve-adrive share download [ranges 1/3]");
        let display = RangeDisplay {
            granularity: ProgressGranularity::Byte,
            ..Default::default()
        };
        let progress = display.begin(3, 20);
        progress.completed(8);
        progress.completed(8);
        assert_eq!(progress.label(), "ve-adrive share download [bytes 16/20]");
    }
}
