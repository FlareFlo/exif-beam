use defmt::{error, Debug2Format};
use embassy_time::{Duration, Timer};
use esp_hal::{uart, Async};
use esp_hal::uart::Uart;
use heapless::Vec;
use ublox::{AnyPacketRef, proto31::Proto31};
use ublox::cfg_val::CfgVal;
use ublox::packets::cfg_val::{CfgLayerSet, CfgValSetBuilder};
use embedded_io_async::Write;
use crate::gps::state::GPS_STATE;

const BAUDRATE_HI: u32 = 19200;

pub fn gps_uart_config(baudrate: u32) -> uart::Config {
    uart::Config::default()
        .with_baudrate(baudrate)
        .with_rx(uart::RxConfig::default().with_fifo_full_threshold(32))
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
	uart.apply_config(&gps_uart_config(BAUDRATE_HI)).unwrap();
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
						AnyPacketRef::Ubx(_ubx_msg) => {
							// info!("Got ubx: {}", Debug2Format(&ubx_msg));
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
										s.has_fix = nmea_state.latitude().is_some();
										drop(s);
									}
									Err(_e) => {
										// debug!("Skipping proprietary or unhandled NMEA sentence");
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
