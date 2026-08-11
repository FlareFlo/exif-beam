use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use chrono::{NaiveDate, NaiveTime};

pub static GPS_STATE: Mutex<CriticalSectionRawMutex, GpsState> = Mutex::new(GpsState::default());

#[derive(Debug)]
pub struct GpsState {
	pub lat: f64,
	pub lon: f64,
	pub sats: u8,
	pub hdop: f32,
	pub time: NaiveTime,
	pub date: NaiveDate,
	pub has_fix: bool,
}

impl GpsState {
	pub const fn default() -> Self {
		Self {
			lat: 0.0,
			lon: 0.0,
			sats: 0,
			hdop: 0.0,
			time: NaiveTime::from_hms_milli_opt(0,0,0,0).unwrap(),
			date: NaiveDate::from_epoch_days(0).unwrap(),
			has_fix: false,
		}
	}
}
