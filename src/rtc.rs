use esp_hal::Async;
use esp_hal::i2c::master::{I2c, Error as I2cError};
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use pcf8563_dd::Pcf8563Async;
use embassy_sync::blocking_mutex::Mutex;
use chrono::{NaiveDateTime, Datelike, Timelike};
use embassy_time::{Duration, Instant, Timer};
use core::cell::Cell;
use esp_hal::gpio::Input;
use crate::gps::wait_for_pps_time;

#[derive(Copy, Clone)]
struct TimeSync {
    rtc_time: Option<NaiveDateTime>,
    sys_time: Option<Instant>,
}

static TIME_SYNC: Mutex<CriticalSectionRawMutex, Cell<TimeSync>> = Mutex::new(Cell::new(TimeSync {
    rtc_time: None,
    sys_time: None,
}));

pub fn get_current_time() -> Option<NaiveDateTime> {
    let sync = TIME_SYNC.lock(|cell| cell.get());
    
    match (sync.rtc_time, sync.sys_time) {
        (Some(rtc_time), Some(sys_time)) => {
            let elapsed: core::time::Duration = sys_time.elapsed().into();
            let chrono_elapsed = chrono::Duration::from_std(elapsed).unwrap_or(chrono::Duration::zero());
            Some(rtc_time + chrono_elapsed)
        }
        _ => None,
    }
}

pub struct RtcTimeSource;
impl embedded_sdmmc::TimeSource for RtcTimeSource {
    fn get_timestamp(&self) -> embedded_sdmmc::Timestamp {
        if let Some(dt) = get_current_time() {
            embedded_sdmmc::Timestamp {
                year_since_1970: (dt.year() - 1970) as u8,
                zero_indexed_month: dt.month0() as u8,
                zero_indexed_day: dt.day0() as u8,
                hours: dt.hour() as u8,
                minutes: dt.minute() as u8,
                seconds: dt.second() as u8,
            }
        } else {
            embedded_sdmmc::Timestamp::from_fat(0, 0)
        }
    }
}

type SharedI2c = I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>;

fn record_time_offset(naive_time: NaiveDateTime) {
    TIME_SYNC.lock(|cell| {
        cell.set(TimeSync {
            rtc_time: Some(naive_time),
            sys_time: Some(Instant::now()),
        });
    });
}

async fn wait_for_gps_time(pps_pin: &mut Input<'static>) -> Option<pcf8563_dd::DateTime> {
    if let Some(accurate_time) = wait_for_pps_time(pps_pin).await {
        record_time_offset(accurate_time);
        Some(pcf8563_dd::DateTime {
            year: (accurate_time.year() % 100) as u8,
            month: accurate_time.month() as u8,
            weekday: accurate_time.weekday().number_from_sunday() as u8,
            day: accurate_time.day() as u8,
            hours: accurate_time.hour() as u8,
            minutes: accurate_time.minute() as u8,
            seconds: accurate_time.second() as u8,
        })
    } else {
        None
    }
}

fn handle_rtc_datetime(dt: &pcf8563_dd::DateTime) {
    let century = 2000;
    if let Some(naive) = chrono::NaiveDate::from_ymd_opt(century + dt.year as i32, dt.month as u32, dt.day as u32)
        .and_then(|d| d.and_hms_opt(dt.hours as u32, dt.minutes as u32, dt.seconds as u32)) {
        record_time_offset(naive);
    }
}

#[embassy_executor::task]
pub async fn drive_rtc(i2c: SharedI2c, mut pps_pin: Input<'static>) {
    let mut rtc = Pcf8563Async::new(i2c);

    let clock_is_valid = rtc.is_clock_valid().await.unwrap_or(false);
    let mut needs_gps_sync = !clock_is_valid;

    loop {
        if needs_gps_sync {
            if let Some(pcf_dt) = wait_for_gps_time(&mut pps_pin).await {
                if rtc.set_datetime(&pcf_dt).await.is_ok() {
                    needs_gps_sync = false;
                }
            } else {
                Timer::after(Duration::from_secs(1)).await;
            }
        } else {
            if let Ok(dt) = rtc.get_datetime().await {
                handle_rtc_datetime(&dt);
            }
            Timer::after(Duration::from_secs(60)).await;
        }
    }
}