mod local_bootstrap;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1).peekable();
    // [Review Fix #103] Keep no-argument local bootstrap compatibility on the single deployment binary.
    let outcome = if arguments.peek().is_none() {
        run_local().map_err(str::to_owned)
    } else {
        ask_auth::platform::run(arguments).map_err(|error| error.to_string())
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn run_local() -> Result<(), &'static str> {
    local_bootstrap::run().await
}
