use embassy_futures::join::join;
use embassy_futures::select::{Either, select};
use esp_radio::ble::controller::BleConnector;
use trouble_host::{
	gatt::GattClient,
};
use trouble_host::prelude::*;

use chrono::{Datelike, Duration as ChronoDuration, NaiveTime, Timelike};

use crate::gps::{GpsState, GPS_STATE};
use crate::tz_data::get_local_offset_seconds;

const SONY_PAIRING_SERVICE_UUID: Uuid = Uuid::new_long([
	0x80, 0x00, 0xEE, 0x00, 0xEE, 0x00, 0xFF, 0xFF,
	0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF
]);
const SONY_PAIRING_WRITE_UUID: Uuid = Uuid::new_short(0xEE01);

const SONY_LOCATION_SERVICE_UUID: Uuid = Uuid::new_long([
	0x80, 0x00, 0xDD, 0x00, 0xDD, 0x00, 0xFF, 0xFF,
	0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF
]);
const SONY_LOCATION_WRITE_UUID: Uuid = Uuid::new_short(0xDD11);
const SONY_LOCATION_CONFIG_UUID: Uuid = Uuid::new_short(0xDD21);
const SONY_LOCATION_LOCK_UUID: Uuid = Uuid::new_short(0xDD30);
const SONY_LOCATION_ENABLE_UUID: Uuid = Uuid::new_short(0xDD31);

// Sony pairing handshake (PROTOCOL.md 2.1): write to characteristic 0xEE01.
const SONY_PAIRING_INIT: [u8; 7] = [0x06, 0x08, 0x01, 0x00, 0x00, 0x00, 0x00];

// Friendly connection params closer to BlueZ than the 80ms/8s default.
fn friendly_conn_params() -> RequestedConnParams {
	RequestedConnParams {
		min_connection_interval: embassy_time::Duration::from_millis(30),
		max_connection_interval: embassy_time::Duration::from_millis(60),
		max_latency: 2,
		min_event_length: embassy_time::Duration::from_secs(0),
		max_event_length: embassy_time::Duration::from_secs(0),
		supervision_timeout: embassy_time::Duration::from_secs(16),
	}
}

// Per whc2001 PROTOCOL_EN.md: time fields are UTC. tz/DST offsets are only
// present when config byte[4] bit 2 is set (then length byte is 0x5D = 93B,
// else 0x59 = 89B).
fn build_payload(state: &GpsState, send_tz: bool) -> [u8; 95] {
	let mut p = [0u8; 95];

	p[0] = 0x00;
	p[1] = if send_tz { 0x5D } else { 0x59 };
	p[2..11].copy_from_slice(&[0x08, 0x02, 0xFC, 0x03, 0x00, 0x00, 0x10, 0x10, 0x10]);

	let lat = (state.lat * 10_000_000.0) as i32;
	let lon = (state.lon * 10_000_000.0) as i32;
	p[11..15].copy_from_slice(&lat.to_be_bytes());
	p[15..19].copy_from_slice(&lon.to_be_bytes());

	// Time fields are UTC.
	p[19..21].copy_from_slice(&(state.date.year() as u16).to_be_bytes());
	p[21] = state.date.month() as u8;
	p[22] = state.date.day() as u8;
	p[23] = state.time.hour() as u8;
	p[24] = state.time.minute() as u8;
	p[25] = state.time.second() as u8;

	if send_tz {
		let utc = state.date.and_time(state.time);
		let ts = utc.and_utc().timestamp();

		let offset_secs = get_local_offset_seconds(state.lat, state.lon, ts).unwrap_or(0);

		let winter_ts = state
			.date
			.with_month(1)
			.map(|d| d.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap()).and_utc().timestamp());
		let std_secs = winter_ts
			.and_then(|t| get_local_offset_seconds(state.lat, state.lon, t))
			.unwrap_or(0);
		let dst_secs = offset_secs - std_secs;

		p[91..93].copy_from_slice(&((std_secs / 60) as i16).to_be_bytes());
		p[93..95].copy_from_slice(&((dst_secs / 60) as i16).to_be_bytes());
	}

	p
}

#[embassy_executor::task]
pub async fn run_ble(
	stack: Stack<'static, ExternalController<BleConnector<'static>, 1>, DefaultPacketPool>
) {
	let mut runner = stack.runner();
	let mut central = stack.central();

	// Camera displayed MAC (from "Disp Device Address") is DC:FE:23:ED:09:E6.
	// BdAddr is stored little-endian (on-air order), so reverse the bytes.
	let target_address =
		Address::new(AddrKind::PUBLIC, BdAddr::new([0xE6, 0x09, 0xED, 0x23, 0xFE, 0xDC]));
	let target_list = [target_address];

	let mut table: trouble_host::prelude::AttributeTable<'_, embassy_sync::blocking_mutex::raw::NoopRawMutex, 16> = trouble_host::prelude::AttributeTable::new();
	let server = trouble_host::prelude::AttributeServer::<_, trouble_host::prelude::DefaultPacketPool, 16, 1>::new(table);

	let _ = join(
		async {
			let _ = runner.run().await;
			unreachable!();
		},
		async {
			let config = ConnectConfig {
				scan_config: ScanConfig {
					filter_accept_list: &target_list,
					..Default::default()
				},
				connect_params: friendly_conn_params(),
			};

			loop {
				let connection = match central.connect(&config).await {
					Ok(c) => c,
					Err(e) => {
						defmt::error!("connect failed: {}", defmt::Debug2Format(&e));
						embassy_time::Timer::after_secs(5).await;
						continue;
					}
				};
				defmt::info!("Sony connected");

				let gatt_conn = match connection.with_attribute_server(&server) {
					Ok(gc) => gc,
					Err(e) => {
						defmt::error!("with_attribute_server failed: {}", defmt::Debug2Format(&e));
						continue;
					}
				};

				let runner_task = async {
					loop {
						use trouble_host::gatt::GattConnectionEvent;
						match gatt_conn.next().await {
							GattConnectionEvent::Disconnected { reason } => {
								defmt::info!("GattConnection disconnected: {}", defmt::Debug2Format(&reason));
								break;
							}
							GattConnectionEvent::Gatt { event } => {
								// auto-processes incoming ATT requests, e.g. from the camera!
								drop(event);
							}
							GattConnectionEvent::PassKeyConfirm(key) => {
								defmt::info!("Camera requires passkey confirm: {}. Auto-confirming...", key);
								let _ = gatt_conn.pass_key_confirm();
							}
							GattConnectionEvent::PassKeyDisplay(key) => {
								defmt::info!("Please type this passkey on the camera: {}", key);
							}
							GattConnectionEvent::PassKeyInput => {
								defmt::info!("Camera wants us to input a passkey. Auto-sending 000000");
								let _ = gatt_conn.pass_key_input(0);
							}
							GattConnectionEvent::PairingFailed(err) => {
								defmt::error!("Pairing failed: {}", defmt::Debug2Format(&err));
							}
							GattConnectionEvent::PairingComplete { security_level, bond } => {
								defmt::info!("Pairing completed successfully!");
							}
							evt => {}
						}
					}
				};

				let client_task = async {
					let connection = gatt_conn.raw();
					let client = match GattClient::<_, _, 10>::new(&stack, &connection).await {
						Ok(c) => c,
						Err(e) => {
							defmt::error!("GattClient init failed: {}", defmt::Debug2Format(&e));
							let _ = connection.disconnect();
							embassy_time::Timer::after_secs(2).await;
							return;
						}
					};

					let services = match client.services().await {
						Ok(s) => s,
						Err(e) => {
							defmt::error!("Service discovery failed: {}", defmt::Debug2Format(&e));
							let _ = connection.disconnect();
							embassy_time::Timer::after_secs(2).await;
							return;
						}
					};
					defmt::info!("Discovered GATT services");

					let _ = connection.set_bondable(true);
					defmt::info!("Waiting 1s before requesting security...");
					embassy_time::Timer::after_secs(1).await;
					defmt::info!("Requesting security...");
					if let Err(e) = connection.request_security() {
						defmt::warn!("request_security returned: {}", defmt::Debug2Format(&e));
					}

					let mut encrypted = false;
					for _ in 0..350 {
						if connection.security_level().map(|l| l.encrypted()).unwrap_or(false) {
							encrypted = true;
							break;
						}
						embassy_time::Timer::after_millis(100).await;
					}

					if !encrypted {
						defmt::error!("Service discovery failed: BleHost(Timeout)");
						return;
					}
					defmt::info!("Connection encrypted!");

					// 1. Sony Pairing Handshake (Service 0xEE00, Char 0xEE01)
					if let Some(pairing_service) = services.iter().find(|s| s.uuid() == SONY_PAIRING_SERVICE_UUID) {
						defmt::info!("Found Sony Pairing Service");
						if let Ok(pairing_char) = client.characteristic_by_uuid(pairing_service, &SONY_PAIRING_WRITE_UUID).await {
							let pairing_char: Characteristic<u8> = pairing_char;
							match client.write_characteristic(&pairing_char, &SONY_PAIRING_INIT).await {
								Ok(_) => defmt::info!("Pairing handshake write successful (0xEE01)"),
								Err(e) => defmt::error!("Pairing write failed: {}", defmt::Debug2Format(&e)),
							}
						} else {
							defmt::warn!("Pairing characteristic 0xEE01 not found");
						}
					}

					// 2. Sony Location Service (Service 0xDD00, Char 0xDD11)
					if let Some(location_service) = services.iter().find(|s| s.uuid() == SONY_LOCATION_SERVICE_UUID) {
						defmt::info!("Found Sony Location Service");
						let location_char: Characteristic<u8> =
							match client.characteristic_by_uuid(location_service, &SONY_LOCATION_WRITE_UUID).await {
								Ok(c) => c,
								Err(e) => {
									defmt::error!("Location characteristic discovery failed: {}", defmt::Debug2Format(&e));
									let _ = connection.disconnect();
									embassy_time::Timer::after_secs(2).await;
									return;
								}
							};

						// Lock + Enable endpoints (fw >= 3.02)
						if let Ok(c) = client.characteristic_by_uuid(location_service, &SONY_LOCATION_LOCK_UUID).await {
							let c: Characteristic<u8> = c;
							let _ = client.write_characteristic(&c, &[0x01]).await;
							defmt::info!("Lock characteristic written");
						}
						if let Ok(c) = client.characteristic_by_uuid(location_service, &SONY_LOCATION_ENABLE_UUID).await {
							let c: Characteristic<u8> = c;
							let _ = client.write_characteristic(&c, &[0x01]).await;
							defmt::info!("Enable characteristic written");
						}

						loop {
							let payload = {
								let state = GPS_STATE.lock().await;
								build_payload(&state, true)
							};
							match client.write_characteristic(&location_char, &payload).await {
								Ok(()) => defmt::info!("Sent location packet"),
								Err(e) => {
									defmt::error!("Location write failed: {}", defmt::Debug2Format(&e));
									break;
								}
							}
							embassy_time::Timer::after_secs(15).await;
						}
					} else {
						defmt::error!("Location service not found in GATT database");
					}

					let _ = connection.disconnect();
					embassy_time::Timer::after_secs(2).await;
				};

				embassy_futures::select::select(client_task, runner_task).await;
			}
		},
	).await;
}
