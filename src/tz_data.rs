use core::hint::black_box;
use defmt::{error, Debug2Format, info};

pub fn get_local_offset_seconds(lat: f64, lon: f64, unix_timestamp: i64) -> Option<i64> {
	let tz_str = trtz::find_tz(lat, lon).or_else(|| {error!("Failed to find tz from gps");  None})?;

	let time_zone = tzdb::tz_by_name(tz_str).or_else(|| {error!("Failed to find tz from name");  None})?;

	let local_time_type = time_zone.find_local_time_type(unix_timestamp).map_err(|e|{error!("Failed to get offset {}", Debug2Format(&e)); e}).ok()?;

	Some(local_time_type.ut_offset() as _)
}