use flexi_logger::{
    default_format, Cleanup, Criterion, Duplicate, FileSpec, Logger, Naming, WriteMode,
};
use log::error;

pub fn init_logger(log_name: &str) {
    if let Err(err) = Logger::try_with_str("trace")
        .unwrap()
        .log_to_file(
            FileSpec::default()
                .directory("logs")
                .basename(log_name)
                .suffix("log"),
        )
        .rotate(
            Criterion::Size(10_000_000),
            Naming::Numbers,
            Cleanup::KeepLogFiles(3),
        )
        .write_mode(WriteMode::Direct)
        .duplicate_to_stderr(Duplicate::Warn)
        .format_for_files(default_format)
        .start()
    {
        error!("Failed to initialize logging: {}", err);
    }
}
