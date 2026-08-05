use embassy_sync::mutex::Mutex;
use core::cell::RefCell;
use defmt::{error, info, debug, Debug2Format};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_time::{Duration, Timer};
use embedded_hal_compat::Reverse;
use embedded_io_async::Write;
use esp_hal::{uart, Async, Blocking};
use esp_hal::i2c::master::I2c;
use esp_hal::uart::Uart;
use heapless::Vec;
use static_cell::StaticCell;
use ublox::{AnyPacketRef, proto31::Proto31};
use ublox::cfg_val::CfgVal;
use ublox::packets::cfg_val::{CfgLayerSet, CfgValSetBuilder};

const BAUDRATE_HI: u32 = 115200;

pub static GPS_STATE: Mutex<CriticalSectionRawMutex, GpsState> = Mutex::new(GpsState::default());

#[derive(Debug)]
pub struct GpsState {
	pub lat: f64,
	pub lon: f64,
	pub sats: u8,
	pub hdop: f32,
}

impl GpsState {
	pub const fn default() -> Self {
		Self {
			lat: 0.0,
			lon: 0.0,
			sats: 0,
			hdop: 0.0,
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