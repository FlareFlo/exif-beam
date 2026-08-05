use core::cell::LazyCell;
use core::ops::DerefMut;
use axp2101_dd::{Axp2101Async, AxpInterface, LdoId};
use axp2101_dd::LdoId::Aldo4;
use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use esp_hal::{i2c::master::{Config as I2cConfig, I2c}, peripherals::I2C0, Async, i2c};

const GPS_LDO: LdoId = Aldo4;

#[derive(Debug)]
pub enum PmicCommand {
	EnableGps(bool),
}

static PMIC_CHANNEL: Channel<CriticalSectionRawMutex, PmicCommand, 4> = Channel::new();
#[embassy_executor::task]
pub async fn run_power_management(
	i2c: I2c<'static, Async>
) {
	let mut pmic = Axp2101Async::new(i2c);

	let receiver = PMIC_CHANNEL.receiver();

	pmic.set_ldo_voltage_mv(GPS_LDO, 3300).await.unwrap();

	loop {
		let cmd = receiver.receive().await;
		match cmd {
			PmicCommand::EnableGps(enable) => {
				if let Err(e) = pmic.set_ldo_enable(GPS_LDO, enable).await {
					defmt::error!("Failed to update GPS power: {:?}", defmt::Debug2Format(&e));
				} else {
					defmt::info!("GPS power rail state changed: {}", enable);
				}
			}
		}
	}
}

pub async fn power_up_gps() {
	PMIC_CHANNEL.send(PmicCommand::EnableGps(true)).await;
}