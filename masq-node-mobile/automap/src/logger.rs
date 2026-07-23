// Copyright (c) 2019-2021, MASQ (https://masq.ai) and/or its affiliates. All rights reserved.

use flexi_logger::{DeferredNow, LevelFilter, LogSpecBuilder, Logger, Record};
use std::env::current_dir;
use std::io;
use std::path::PathBuf;

pub fn initiate_logger() -> Result<(), String> {
    let working_path = resolve_working_path(current_dir())?;
    let logger = Logger::with(LogSpecBuilder::new().default(LevelFilter::Info).build())
        .log_to_file()
        .directory(working_path.as_path())
        .format(brief_format)
        .print_message()
        .suppress_timestamp();

    logger
        .start()
        .map(|_| ())
        .map_err(|_| "Automap logging subsystem could not start".to_string())
}

fn resolve_working_path(result: io::Result<PathBuf>) -> Result<PathBuf, String> {
    result.map_err(|_| "Automap working directory is unavailable".to_string())
}

fn brief_format(
    w: &mut dyn std::io::Write,
    _now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    write!(w, "{}:   {}", record.level(), record.args())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_working_path_reports_an_os_failure_without_panicking_or_retaining_details() {
        let result = resolve_working_path(Err(io::Error::new(
            io::ErrorKind::Other,
            "SENSITIVE_WORKING_DIRECTORY_DETAIL",
        )));

        assert_eq!(
            result,
            Err("Automap working directory is unavailable".to_string())
        );
    }
}
