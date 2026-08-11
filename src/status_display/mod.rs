use crate::tz_data::get_local_offset_seconds;
use core::sync::atomic::AtomicBool;
use embassy_sync::mutex::Mutex;
use display_interface_i2c::I2CInterface;
use oled_async::displays::sh1106::Sh1106_128_64;
use oled_async::Builder;
use oled_async::prelude::GraphicsMode;
use embedded_hal_compat::{ForwardCompat, Reverse};
use core::cell::RefCell;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc};
use chrono::{FixedOffset, Timelike};
use core::fmt::Debug;
use core::cmp::min;
use core::ops::Not;
use core::fmt::Write;
use core::sync::atomic::Ordering;
use chrono::format::Fixed;
use defmt::{info, Debug2Format};
use embassy_futures::select::{select, select4, Either4};
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex};
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::Drawable;
use embassy_sync::signal::Signal;
use embedded_graphics::geometry::Size;
use embedded_graphics::image::{Image, ImageRaw};
use embedded_graphics::mono_font::{MonoFont};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::mono_font::ascii::*;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, Primitive};
use embedded_graphics::prelude::Point;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Alignment, Baseline, TextStyleBuilder};
use embedded_graphics::text::Text;
use esp_hal::{Async, Blocking};
use esp_hal::i2c::master::I2c;
use heapless::String;
use oled_async::displayrotation::DisplayRotation;
use crate::gps::GPS_STATE;
use crate::power_management::{get_power_state, PowerState, POWER_BUTTON_CHANNEL, PowerButtonEvent};
use embedded_graphics::geometry::Dimensions;

pub mod widgets;
use widgets::battery::draw_battery_status;
use widgets::location::draw_location;
use widgets::time::draw_time;
use widgets::status::{draw_16_16, BoxLevel};

pub static IS_DISPLAY_AWAKE: AtomicBool = AtomicBool::new(true);
pub static DISPLAY_SIGNAL: Signal<CriticalSectionRawMutex, DisplayState> = Signal::new();
pub static DISPLAY_ROTATION_SIGNAL: Signal<CriticalSectionRawMutex, DisplayRotation> = Signal::new();

#[embassy_executor::task]
pub async fn drive_display(bus_ref: &'static Mutex<CriticalSectionRawMutex, I2c<'static, Async>>) {
	let i2cd = I2cDevice::new(bus_ref);
	let interface = I2CInterface::new(i2cd, 0x3D, 0x40);
	let mut display: GraphicsMode<_, _> = Builder::new(Sh1106_128_64 {})
		.with_rotation(DisplayRotation::Rotate0)
		.connect(interface)
		.into();

	display.init().await.unwrap();
	display.clear();
	display.flush().await.unwrap();

	let mut state = DisplayState::default();
	let mut power_sub = POWER_BUTTON_CHANNEL.subscriber().unwrap();
	let mut last_interaction = Instant::now();
	let mut is_asleep = false;

	loop {
		let res = select4(
			Timer::after(Duration::from_secs(1)),
			DISPLAY_SIGNAL.wait(),
			power_sub.next_message_pure(),
			DISPLAY_ROTATION_SIGNAL.wait()
		).await;

		match res {
			Either4::Third(event) => {
				if event == PowerButtonEvent::ShortPress {
					last_interaction = Instant::now();
					if is_asleep {
						display.display_on(true).await.unwrap();
						is_asleep = false;
						IS_DISPLAY_AWAKE.store(true, Ordering::Relaxed);
					}
				}
			}
			Either4::Fourth(rot) => {
				display.set_rotation(rot).await.unwrap();
				display.clear(); // Clear display because drawing bounds might change
			}
			_ => {}
		}

		if !is_asleep && last_interaction.elapsed() > Duration::from_secs(60) {
			display.display_on(false).await.unwrap();
			is_asleep = true;
			IS_DISPLAY_AWAKE.store(false, Ordering::Relaxed);
		}

		if !is_asleep {
			let gps = GPS_STATE.lock().await;
			state.sats = gps.sats;
			state.lat = gps.lat;
			state.lon = gps.lon;
			state.date = gps.date;
			state.time = gps.time;
			state.hdop = gps.hdop;
			drop(gps);
			if state.tz_offset.is_none() && state.lat != 0.0 {
				let dt = NaiveDateTime::new(state.date, state.time);
				state.tz_offset = Some(get_local_offset_seconds(state.lat, state.lon, dt.and_utc().timestamp()).unwrap());
			}
			state.power = get_power_state();
			draw_status_display(&mut display, &state);
			yield_now().await;
			display.flush().await.unwrap();
		}
	}
}

#[derive(Clone, Default)]
pub struct DisplayState {
	time: NaiveTime,
	date: NaiveDate,
	pub local_time: Option<DateTime<FixedOffset>>,
	pub lat: f64,
	pub lon: f64,
	pub sats: u8,
	pub hdop: f32,
	pub power: PowerState,
	pub tz_offset: Option<i64>,
}

impl DisplayState {
	//
	pub fn update_date(&mut self, d: NaiveDate) {
		self.date = d;
	}

	/// This must be GPS UTC time!
	pub fn update_utc_time(&mut self, t: NaiveTime) {
		self.time = t;
	}

	pub fn now_utc(&self) -> DateTime<FixedOffset> {
		NaiveDateTime::new(self.date, self.time).and_utc().fixed_offset()
	}

	pub fn now_local(&self) -> Option<DateTime<FixedOffset>> {
		let dt = NaiveDateTime::new(self.date, self.time);
		Some(dt.and_utc().fixed_offset().checked_add_signed(TimeDelta::try_seconds(self.tz_offset?).unwrap()).unwrap())
	}
}

fn draw_status_display<D>(display: &mut D, state: &DisplayState)
where
	D: DrawTarget<Color = BinaryColor>,
	D::Error: Debug,
{
	display.clear(BinaryColor::Off).unwrap();
	let blink = state.time.second() % 2 == 1;

	let size = display.bounding_box().size;
	let is_landscape = size.width > size.height;

	let (now, is_utc) = if let Some(time) = state.now_local() {
		(time, false)
	} else {
		(state.now_utc() , true)
	};

	let hdop_level = match state.hdop {
		0.1..2.0 => BoxLevel::Info,
		2.0..5.0 => BoxLevel::Info,
		5.0..20.0 => BoxLevel::Warn,
		20.0.. | 0.0 => BoxLevel::Error,
		_ => BoxLevel::Error,
	};
	let hdop_l1 = match state.hdop {
		0.1..2.0 => "EXC",
		2.0..5.0 => "OKY",
		5.0..20.0 => "POR",
		20.0.. | 0.0 => "NO",
		_ => "???",
	};

	if is_landscape {
		// 2:1 Aspect Ratio (e.g., 128x64)
		draw_location(display, Point::new(0, 0), state.lat, state.lon, &FONT_6X13).unwrap();
		draw_time(display, Point::new(0, size.height as i32 - 13), &now, is_utc, &FONT_6X13_BOLD).unwrap();
		draw_16_16(hdop_l1, "FIX", Point::new(54, 0), hdop_level, display, blink);
		draw_battery_status(display, Point::new(size.width as i32 - 22, 0), state.power).unwrap();
	} else {
		// 1:2 Aspect Ratio (e.g., 64x128)
		draw_battery_status(display, Point::new(size.width as i32 - 22, 0), state.power).unwrap();
		draw_16_16(hdop_l1, "FIX", Point::new(0, 0), hdop_level, display, blink);
		draw_time(display, Point::new(0, 18), &now, is_utc, &FONT_6X13_BOLD).unwrap();
		// Location goes at the bottom
		draw_location(display, Point::new(0, size.height as i32 - 26), state.lat, state.lon, &FONT_6X13).unwrap();
	}
}