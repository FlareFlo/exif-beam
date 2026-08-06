use embassy_futures::join::join;
use esp_radio::ble::controller::BleConnector;
use trouble_host::{
	gatt::GattClient,
};
use trouble_host::prelude::*;

const SONY_SERVICE_UUID: Uuid = Uuid::new_long([
	0x80, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0xFF,
	0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF
]);
const SONY_CMD_UUID: Uuid = Uuid::new_short(0xFF01);

#[embassy_executor::task]
pub async fn run_ble(
	stack: Stack<'static, ExternalController<BleConnector<'static>, 1>, DefaultPacketPool>
) {
	let mut central = stack.central();

	let scan_config = trouble_host::prelude::ScanConfig::default();
	let target_address = Address::new(AddrKind::RANDOM, BdAddr::new([0xDC, 0xFE, 0x23, 0xED, 0x09, 0xE6]));
	let target_list = [target_address];

	let config = ConnectConfig {
		scan_config: ScanConfig {
			filter_accept_list: &target_list,
			..Default::default()
		},
		connect_params: Default::default(),
	};

	let connection = central.connect(&config).await.unwrap();

	connection.request_security().unwrap();

	let client = GattClient::<_, _, 10>::new(&stack, &connection).await.unwrap();

	let _ = join(
		client.task(),
		async {
			let services = client.services_by_uuid(&SONY_SERVICE_UUID).await.unwrap();
			let sony_service = services.first().expect("Sony service not found");

			let cmd_char: Characteristic<u8> = client.characteristic_by_uuid(sony_service, &SONY_CMD_UUID).await.unwrap();

			client.write_characteristic(&cmd_char, &[0x01, 0x07]).await.unwrap();
			embassy_time::Timer::after_millis(200).await;

			client.write_characteristic(&cmd_char, &[0x01, 0x09]).await.unwrap();
			embassy_time::Timer::after_millis(50).await;

			client.write_characteristic(&cmd_char, &[0x01, 0x08]).await.unwrap();
			client.write_characteristic(&cmd_char, &[0x01, 0x06]).await.unwrap();
		}
	).await;
}