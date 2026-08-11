use chrono::{Datelike, Timelike};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embedded_sdmmc::{TimeSource, Timestamp};
use crate::gps::GPS_STATE;

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum SdStatus {
    Missing,
    Idle,
    Recording,
    Error,
}

pub static SD_STATE: Mutex<CriticalSectionRawMutex, SdStatus> = Mutex::new(SdStatus::Missing);

pub struct GpsTimeSource;

impl TimeSource for GpsTimeSource {
    fn get_timestamp(&self) -> Timestamp {
        if let Ok(gps) = GPS_STATE.try_lock() {
            Timestamp {
                year_since_1970: (gps.date.year().saturating_sub(1970).clamp(0, 255)) as u8,
                zero_indexed_month: gps.date.month0() as u8,
                zero_indexed_day: gps.date.day0() as u8,
                hours: gps.time.hour() as u8,
                minutes: gps.time.minute() as u8,
                seconds: gps.time.second() as u8,
            }
        } else {
            Timestamp::from_fat(0, 0)
        }
    }
}
