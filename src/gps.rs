use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use core::cell::RefCell;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering::Relaxed;
use chrono::{DateTime, NaiveDate, NaiveTime, Datelike};
use defmt::{error, info, debug, Debug2Format};
use embassy_executor::Spawner;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use embedded_hal_compat::eh0_2::digital::v2::InputPin;
use embedded_hal_compat::Reverse;
use embedded_io_async::Write;
use esp_hal::{uart, Async, Blocking};
use esp_hal::gpio::{AnyPin, Input, InputConfig, Level, Pull};
use esp_hal::i2c::master::I2c;
use esp_hal::uart::Uart;
use heapless::Vec;
use static_cell::StaticCell;
use ublox::{AnyPacketRef, proto31::Proto31};
use ublox::cfg_val::CfgVal;
use ublox::packets::cfg_val::{CfgLayerSet, CfgValSetBuilder};

const BAUDRATE_HI: u32 = 19200;

pub static GPS_STATE: Mutex<CriticalSectionRawMutex, GpsState> = Mutex::new(GpsState::default());

#[derive(Debug)]
pub struct GpsState {
	pub lat: f64,
	pub lon: f64,
	pub sats: u8,
	pub hdop: f32,
	pub time: NaiveTime,
	pub date: NaiveDate,
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
		}
	}
}

#[embassy_executor::task]
pub async fn run_gps(mut uart: Uart<'static, Async>) {
	let mut uart_rx = [0u8; 512];
	let mut parser = ublox::ParserBuilder::new()
		.with_protocol::<Proto31>()
		.with_fixed_buffer::<1024>();

	// Reconfigure baudrate
	let mut config: Vec<u8, 32> = Vec::new();
	CfgValSetBuilder {
		version: 1,
		layers: CfgLayerSet::RAM,
		reserved1: 0,
		cfg_data: &[CfgVal::Uart1Baudrate(BAUDRATE_HI)]
	}.extend_to(&mut config);
	uart.write_all(config.as_slice()).await.unwrap();
	uart.flush_async().await.unwrap();
	Timer::after(Duration::from_millis(50)).await;
	uart.apply_config(&uart::Config::default().with_baudrate(BAUDRATE_HI)).unwrap();
	Timer::after(Duration::from_millis(50)).await;

	let mut nmea_state = nmea::Nmea::default();

	loop {
		let read = match uart.read_async(&mut uart_rx).await {
			Ok(n) => n,
			Err(e) => {
				error!("UART read error: {:?}", Debug2Format(&e));
				continue;
			}
		};

		if read == 0 {
			continue;
		}

		let mut it = parser.consume_ubx_rtcm_nmea(&uart_rx[..read]);
		loop {
			match it.next() {
				Some(Ok(packet)) => {
					match packet {
						AnyPacketRef::Ubx(ubx_msg) => {
							info!("Got ubx: {}", Debug2Format(&ubx_msg));
						}
						AnyPacketRef::Rtcm(_rtcm_msg) => {}
						AnyPacketRef::Nmea(msg) => {
							if let Ok(sentence_str) = core::str::from_utf8(msg.data) {
								match nmea_state.parse(sentence_str) {
									Ok(_) => {
										let mut s = GPS_STATE.lock().await;
										s.lat = nmea_state.latitude().unwrap_or_default() as _;
										s.lon = nmea_state.longitude().unwrap_or_default() as _;
										s.sats = nmea_state.satellites().len() as u8;
										s.time = nmea_state.fix_timestamp().unwrap_or_default();
										s.date = nmea_state.fix_date.unwrap_or_default();
										s.hdop = nmea_state.hdop().unwrap_or_default();
										drop(s);
									}
									Err(_e) => {
										debug!("Skipping proprietary or unhandled NMEA sentence");
									}
								}
							}
						}
					}
				}
				Some(Err(e)) => {
					error!("Parser protocol error: {:?}", Debug2Format(&e));
				}
				None => {
					break;
				}
			}
		}
	}
}

#[embassy_executor::task]
pub async fn gpx_logger(pin: AnyPin<'static>, spawner: Spawner) {
	let pin = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
	let mut button = async_button::Button::new(pin, async_button::ButtonConfig::default());

	static signal: AtomicBool = AtomicBool::new(false);

	spawner.spawn(run_gps_logger(&signal).unwrap());
	loop {
		match button.update().await {
			async_button::ButtonEvent::ShortPress { count } => {
				info!("Button short pressed {} times!", count);
				signal.store(true, Relaxed);
			}
			async_button::ButtonEvent::LongPress => {
				info!("Button long pressed!");
				signal.store(false, Relaxed);
			}
		}
	}
}

#[embassy_executor::task(pool_size = 1)]
async fn run_gps_logger(run: &'static AtomicBool) {
	loop {
		// Run
		while run.load(Relaxed) {
			info!("TOOD: write GPS logs somewhere idk");
			// TODO: Init SD card and memory ringbuf, write GPX formatted logs
			Timer::after_millis(2000).await;
		}
		// Cleanup. Should probaably put the GPS into PCMT mode here
		Timer::after_millis(500).await;
	}
}

pub async fn wait_for_pps_time(pps_pin: &mut Input<'_>) -> Option<chrono::NaiveDateTime> {
    let s = GPS_STATE.lock().await;
    let date = s.date;
    let time = s.time;
    drop(s);

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