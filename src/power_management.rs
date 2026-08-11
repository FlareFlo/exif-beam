use crate::i2c::master::Error;
use core::cell::LazyCell;
use core::ops::DerefMut;
use core::sync::atomic::{AtomicU8, AtomicBool, Ordering};
use axp2101_dd::{Axp2101Async, AxpInterface, FastChargeCurrentLimit, LdoId, VoffVoltage};
use axp2101_dd::LdoId::*;
use critical_section::Mutex;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_futures::select::Either3;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::pubsub::PubSubChannel;
use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;
use esp_hal::{i2c::master::{Config as I2cConfig, I2c}, peripherals::I2C0, Async, i2c};
use esp_hal::peripherals::I2C1;
use crate::status_display::{DISPLAY_SIGNAL, DisplayState};

const GPS_LDO: LdoId = Aldo4;
const AUX_LDO: LdoId = Aldo1;

const IMU_LDO: LdoId = Aldo2;

// AXP2101 IRQ1 Masks (VBUS, Battery insertion, Power Key)
const IRQ1_VBUS_INSERT: u8 = 0x80;
const IRQ1_VBUS_REMOVE: u8 = 0x40;
const IRQ1_BAT_INSERT: u8 = 0x20;
const IRQ1_BAT_REMOVE: u8 = 0x10;
const IRQ1_PWR_SHORT: u8 = 0x08;
const IRQ1_PWR_LONG: u8 = 0x04;
const IRQ1_PWR_NEG: u8 = 0x02;
const IRQ1_PWR_POS: u8 = 0x01;

// AXP2101 IRQ2 Masks (Charging state and faults)
const IRQ2_CHG_DONE: u8 = 0x10;
const IRQ2_CHG_STATE: u8 = 0x08;

static BAT_LEVEL: AtomicU8 = AtomicU8::new(255);
static BAT_PRESENT: AtomicBool = AtomicBool::new(true);
static VBUS_GOOD: AtomicBool = AtomicBool::new(false);
static CHARGING: AtomicBool = AtomicBool::new(false);

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PowerState {
	Battery(u8),
	Charging(u8),
	VusbOnly,
	Unknown,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PowerButtonEvent {
	ShortPress,
	LongPress,
}

// Just 4 tasks can wait for their message. Add more if needed.
pub static POWER_BUTTON_CHANNEL: PubSubChannel<CriticalSectionRawMutex, PowerButtonEvent, 4, 4, 1> = PubSubChannel::new();

impl Default for PowerState {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug)]
pub enum PmicCommand {
	EnableGps(bool),
	EnableAux(bool),
    EnableImu(bool),
}

static PMIC_CHANNEL: Channel<CriticalSectionRawMutex, PmicCommand, 4> = Channel::new();
#[embassy_executor::task]
pub async fn run_power_management(
	i2c: I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>,
	mut irq_pin: Input<'static>
) {
	let mut pmic = Axp2101Async::new(i2c);

	let receiver = PMIC_CHANNEL.receiver();

	pmic.set_ldo_voltage_mv(AUX_LDO, 3300).await.unwrap();
	pmic.set_ldo_voltage_mv(GPS_LDO, 3300).await.unwrap();
	pmic.set_ldo_voltage_mv(IMU_LDO, 3300).await.unwrap();

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

	// Enable hardware interrupts for power state changes so we don't have to poll aggressively
	// IRQ1: We care about VBUS (USB plugged/unplugged), Battery physical insertion/removal, and Power Button presses
	let irq1_mask = IRQ1_VBUS_INSERT | IRQ1_VBUS_REMOVE | IRQ1_BAT_INSERT | IRQ1_BAT_REMOVE | IRQ1_PWR_SHORT | IRQ1_PWR_LONG;
	if let Err(e) = pmic.enable_interrupts(0x00, irq1_mask).await {
		defmt::error!("Failed to enable IRQ1: {:?}", defmt::Debug2Format(&e));
	}
	
	// IRQ2: We care about charging state changes (started charging, finished charging)
	let irq2_mask = IRQ2_CHG_STATE | IRQ2_CHG_DONE;
	if let Err(e) = pmic.enable_interrupts2(irq2_mask).await {
		defmt::error!("Failed to enable IRQ2: {:?}", defmt::Debug2Format(&e));
	}
	let _ = pmic.clear_interrupt_status().await;
	let _ = pmic.clear_interrupt_status2().await;

	let unneeded = [
		Aldo3, // LoRa Radio
		Bldo1, // SD card
		Bldo2  // External header
	];
	for rail in unneeded {
		pmic.set_ldo_enable(rail, false).await.unwrap();
	}

	loop {
		let cmd = if irq_pin.is_low() {
			Either3::Third(())
		} else {
			embassy_futures::select::select3(
				receiver.receive(),
				Timer::after(Duration::from_secs(10)),
				irq_pin.wait_for_falling_edge(),
			).await
		};
		match cmd {
			Either3::First(cmd) => {
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
                    PmicCommand::EnableImu(enable) => {
						if let Err(e) = pmic.set_ldo_enable(IMU_LDO, enable).await {
							defmt::error!("Failed to update IMU power: {:?}", defmt::Debug2Format(&e));
						} else {
							defmt::info!("IMU power rail state changed: {}", enable);
						}
                    }
				}
			}
			Either3::Second(_) | Either3::Third(_) => {
				if matches!(cmd, Either3::Third(_)) {
					if let Ok((irq1,)) = pmic.get_interrupt_status1().await {
						if irq1 & IRQ1_PWR_LONG != 0 {
							defmt::warn!("Long power button press");
							if let Ok(publisher) = POWER_BUTTON_CHANNEL.publisher() {
								publisher.publish_immediate(PowerButtonEvent::LongPress);
							}
						} else if irq1 & IRQ1_PWR_SHORT != 0 {
							defmt::info!("Short power button press");
							if let Ok(publisher) = POWER_BUTTON_CHANNEL.publisher() {
								publisher.publish_immediate(PowerButtonEvent::ShortPress);
							}
						}
					}

					let _ = pmic.clear_interrupt_status().await;
					let _ = pmic.clear_interrupt_status1().await;
					let _ = pmic.clear_interrupt_status2().await;
				}

				if let Ok(power) = pmic.get_battery_level().await {
					BAT_LEVEL.store(power, Ordering::Relaxed);
				}
				if let Ok(status) = pmic.get_power_status().await {
					VBUS_GOOD.store(status.0, Ordering::Relaxed);
					BAT_PRESENT.store(status.2, Ordering::Relaxed);
				}
				if let Ok(charging) = pmic.is_charging().await {
					CHARGING.store(charging, Ordering::Relaxed);
				}
				
				if matches!(cmd, Either3::Third(_)) {
					DISPLAY_SIGNAL.signal(DisplayState::default());
				}
			}
		}
	}
}

pub fn get_power_state() -> PowerState {
	let level = BAT_LEVEL.load(Ordering::Relaxed);
	let present = BAT_PRESENT.load(Ordering::Relaxed);
	let vbus = VBUS_GOOD.load(Ordering::Relaxed);
	let charging = CHARGING.load(Ordering::Relaxed);
	
	if vbus && !present {
		PowerState::VusbOnly
	} else if charging {
		PowerState::Charging(if level <= 100 { level } else { 0 })
	} else if present && level <= 100 {
		PowerState::Battery(level)
	} else {
		PowerState::Unknown
	}
}

pub async fn power_up_gps() {
	PMIC_CHANNEL.send(PmicCommand::EnableGps(true)).await;
}

pub async fn power_up_aux() {
	PMIC_CHANNEL.send(PmicCommand::EnableAux(true)).await;
}

pub async fn power_up_imu() {
    PMIC_CHANNEL.send(PmicCommand::EnableImu(true)).await;
}
