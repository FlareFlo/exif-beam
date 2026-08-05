use defmt::{error, info, debug, Debug2Format};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use esp_hal::{uart, Async};
use esp_hal::uart::Uart;
use heapless::Vec;
use ublox::{AnyPacketRef, proto31::Proto31};
use ublox::cfg_val::CfgVal;
use ublox::packets::cfg_val::{CfgLayerSet, CfgValSetBuilder};

const BAUDRATE_HI: u32 = 115200;

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
						AnyPacketRef::Ubx(_ubx_msg) => {}
						AnyPacketRef::Rtcm(_rtcm_msg) => {}
						AnyPacketRef::Nmea(msg) => {
							// Convert message bytes back to a string slice for the nmea crate
							if let Ok(sentence_str) = core::str::from_utf8(msg.data) {
								match nmea_state.parse(sentence_str) {
									Ok(_) => {
										// 3. Access unified state fields directly from `nmea_state`
										info!(
                                            "GPS State Update -> Lat: {:?}, Lon: {:?}, Alt: {:?}m, Fix: {:?}",
                                            nmea_state.latitude,
                                            nmea_state.longitude,
                                            nmea_state.altitude,
                                            nmea_state.fix_type
                                        );
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