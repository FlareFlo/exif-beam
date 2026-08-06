use crate::i2c::master::Error;
use core::cell::LazyCell;
use core::ops::DerefMut;
use core::sync::atomic::{AtomicU8, Ordering};
use axp2101_dd::{Axp2101Async, AxpInterface, FastChargeCurrentLimit, LdoId, VoffVoltage};
use axp2101_dd::LdoId::*;
use critical_section::Mutex;
use embassy_futures::select::Either;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use esp_hal::{i2c::master::{Config as I2cConfig, I2c}, peripherals::I2C0, Async, i2c};

const GPS_LDO: LdoId = Aldo4;
const AUX_LDO: LdoId = Aldo1;

static BAT: AtomicU8 = AtomicU8::new(0);

#[derive(Debug)]
pub enum PmicCommand {
	EnableGps(bool),
	EnableAux(bool),
}

static PMIC_CHANNEL: Channel<CriticalSectionRawMutex, PmicCommand, 4> = Channel::new();
#[embassy_executor::task]
pub async fn run_power_management(
	i2c: I2c<'static, Async>
) {
	let mut pmic = Axp2101Async::new(i2c);

	let receiver = PMIC_CHANNEL.receiver();

	pmic.set_ldo_voltage_mv(AUX_LDO, 3300).await.unwrap();
	pmic.set_ldo_voltage_mv(GPS_LDO, 3300).await.unwrap();
	//##################################################################################
	// =/=/=/=/=/=/=/=/=/=/ WARNING WARNING WARNING =/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/
	// I'm using the Samsung 18650 26J, a LIcoO2 chemistry that allows up to 2.75V under discharge.
	// I rather not risk anything and choose conservative 3V (losing about 3-5% capacity)
	// MAKE SURE YOUR BATTERY SUPPORTS THIS VOLTAGE OR OTHERWISE CHANGE IT!!!!!!
	// 3.3V is probably always safe for lithium architectures, no promises though
	// =/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/=/
	//##################################################################################
	pmic.battery_discharge_limit(VoffVoltage::V30).await.unwrap();
	pmic.set_battery_charge_current(FastChargeCurrentLimit::Ma1000).await.unwrap();

	disable_unneeded_rails(&mut pmic).await;

	loop {
		let cmd = embassy_futures::select::select(
			receiver.receive(),
			Timer::after(Duration::from_secs(10)),
		).await;
		match cmd {
			Either::First(cmd) => {
				match cmd {
					PmicCommand::EnableGps(enable) => {
						if let Err(e) = pmic.set_ldo_enable(GPS_LDO, enable).await {
							defmt::error!("Failed to update GPS power: {:?}", defmt::Debug2Format(&e));
						} else {
							defmt::info!("GPS power rail state changed: {}", enable);
						}
					}
					PmicCommand::EnableAux(enable) => {
						if let Err(e) = pmic.set_ldo_enable(AUX_LDO, enable).await {
							defmt::error!("Failed to update AUX power: {:?}", defmt::Debug2Format(&e));
						} else {
							defmt::info!("AUX power rail state changed: {}", enable);
						}
					}
				}
			}
			Either::Second(_) => {
				if let Ok(power) = pmic.get_battery_level().await {
					BAT.store(power, Ordering::Relaxed);
				}
			}
		}
	}
}

pub fn get_battery_level() -> u8 {
	BAT.load(Ordering::Relaxed)
}

pub async fn power_up_gps() {
	PMIC_CHANNEL.send(PmicCommand::EnableGps(true)).await;
}

pub async fn power_up_aux() {
	PMIC_CHANNEL.send(PmicCommand::EnableAux(true)).await;
}

async fn disable_unneeded_rails(pmic: &mut Axp2101Async<AxpInterface<I2c<'static, Async>>, Error>) {
	let unneeded = [
		Aldo2, // IMU
		Aldo3, // LoRa Radio
		Bldo1, // SD card
		Bldo2  // External header
	];
	for rail in unneeded {
		pmic.set_ldo_enable(rail, false).await.unwrap();
	}
}