use embassy_time::Duration;
use esp_hal::gpio::Input;

use crate::gps::state::GPS_STATE;

pub async fn wait_for_pps_time(pps_pin: &mut Input<'_>) -> Option<chrono::NaiveDateTime> {
    let s = GPS_STATE.lock().await;
    let date = s.date;
    let time = s.time;
    drop(s);

    use chrono::Datelike;
    if date.year() <= 1970 {
        return None; // Time not yet acquired by GPS
    }

    let dt = chrono::NaiveDateTime::new(date, time);
    // Add 1 second because PPS indicates the start of the next second
    let upcoming_second = dt + chrono::Duration::seconds(1);

    let pps_future = pps_pin.wait_for_rising_edge();
    match embassy_time::with_timeout(Duration::from_millis(1200), pps_future).await {
        Ok(_) => Some(upcoming_second),
        Err(_) => None,
    }
}
