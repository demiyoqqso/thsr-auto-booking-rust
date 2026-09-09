use clap::Parser;
use clap::builder::TypedValueParser;

/// A CLI tool for booking Taiwan High Speed Rail tickets.
#[derive(Parser, Debug, Clone)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Personal ID
    #[arg(long, short = 'i', value_name = "ID", env = "THSR_PERSONAL_ID")]
    pub personal_id: Option<String>,

    /// Departure date
    #[arg(long, short = 'd', value_name = "DATE", env = "THSR_DATE")]
    pub date: Option<String>,

    /// Time ID of the departure time
    #[arg(long, short = 'T', value_name = "TIME_ID", env = "THSR_TIME")]
    pub time: Option<usize>,

    /// Departure station ID
    #[arg(long, short = 'f', value_name = "STATION_ID", env = "THSR_FROM")]
    pub from: Option<usize>,

    /// Arrival station ID
    #[arg(long, short = 't', value_name = "STATION_ID", env = "THSR_TO")]
    pub to: Option<usize>,

    /// Number of adults
    #[arg(long, short = 'a', value_name = "NUMBER", env = "THSR_ADULT_CNT")]
    pub adult_cnt: Option<u8>,

    /// Number of students
    #[arg(long, short = 's', value_name = "NUMBER", env = "THSR_STUDENT_CNT")]
    pub student_cnt: Option<u8>,

    /// Seat preference. 0: None, 1: Window, 2: Aisle
    #[arg(
        long,
        short = 'p',
        value_name = "NUMBER",
        env = "THSR_SEAT_PREFER",
        value_parser = clap::builder::PossibleValuesParser::new(["0", "1", "2"])
            .map(|s| s.parse::<usize>().unwrap())
    )]
    pub seat_prefer: Option<usize>,

    /// Class type. 0: Standard, 1: Business
    #[arg(
        long,
        short = 'c',
        value_name = "NUMBER",
        env = "THSR_CLASS_TYPE",
        value_parser = clap::builder::PossibleValuesParser::new(["0", "1"])
            .map(|s| s.parse::<usize>().unwrap())
    )]
    pub class_type: Option<usize>,

    /// Whether to use personal ID as membership
    #[arg(long, short = 'm', value_name = "BOOL", env = "THSR_USE_MEMBERSHIP")]
    pub use_membership: Option<bool>,

    /// Retry interval in seconds when no train is available
    #[arg(long, default_value_t = 3, env = "THSR_RETRY_SECONDS")]
    pub retry_seconds: u64,

    #[arg(long)]
    pub list_station: bool,

    #[arg(long)]
    pub list_time_table: bool,
}
