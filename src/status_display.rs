use crate::tz_data::get_local_offset_seconds;
use AtomicBool;
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
	const GPS_FONT: MonoFont = FONT_6X13;
	const TXT_B: MonoTextStyleBuilder<BinaryColor> = MonoTextStyleBuilder::new().font(&GPS_FONT);
	const TXT: MonoTextStyleBuilder<BinaryColor> = TXT_B.text_color(BinaryColor::On);
	const TXT_INV: MonoTextStyleBuilder<BinaryColor> = TXT_B.text_color(BinaryColor::Off).background_color(BinaryColor::On);
	const fn yoffs(i: u8) -> i32 {
		let neg_margin = 2;
		-neg_margin + (GPS_FONT.character_size.height as i32 - neg_margin) * i as i32
	}

	display.clear(BinaryColor::Off).unwrap();
	let blink = state.time.second() % 2 == 1;

	// Lat
	Text::with_baseline(
		&heapless::format!(10; "N{:>8.5}", state.lat).unwrap(),
		Point::new(0, yoffs(0)),
		TXT.build(),
		Baseline::Top,
	)
		.draw(display)
		.unwrap();

	// Lon
	Text::with_baseline(
		&heapless::format!(10; "E{:>8.5}", state.lon).unwrap(),
		Point::new(0, yoffs(1)),
		TXT.build(),
		Baseline::Top,
	)
		.draw(display)
		.unwrap();

	// TODO: This somehow still prints the wrong TZ atm
	let (now, is_utc) = if let Some(time) = state.now_local() {
		(time, false)
	} else {
		(state.now_utc() , true)
	};
	// Clock
	Text::with_baseline(&heapless::format!(30; " {:02}:{:02}:{:02}", now.hour(), now.minute(), now.second()).unwrap(), Point::new(0, yoffs(2) + 1), TXT.font(&FONT_6X13_BOLD).build(), Baseline::Top)
		.draw(display)
		.unwrap();

	Image::new(&if is_utc.not() {LOC_90DEG } else { UTC_90DEG }, Point::new(0, 21)).draw(display).unwrap();
	match state.hdop {
		0.1..2.0 => {
			draw_16_16("EXC", "FIX", Point::new(54,0), BoxLevel::Info, display, blink);
		}
		2.0..5.0 => {
			draw_16_16("OKY", "FIX", Point::new(54,0), BoxLevel::Info, display, blink);
		}
		5.0..20.0 => {
			draw_16_16("POR", "FIX", Point::new(54,0), BoxLevel::Warn, display, blink);
		}
		20.0.. | 0.0 => {
			draw_16_16("NO", "FIX", Point::new(54,0), BoxLevel::Error, display, blink);
		}
		_ => {
			draw_16_16("???", "FIX", Point::new(54,0), BoxLevel::Error, display, blink);
		}
	}

	// draw_16_16("BAD", "FIX", Point::new(54,16), blink, display);
	// draw_16_16("NO", "PPS", Point::new(54 + 16,0), blink, display);
	// draw_16_16("BAD", "PPS", Point::new(54 + 16,16), blink, display);
	draw_battery_status(display, Point::new(128 - 22, 0), state.power).unwrap();
}

#[derive(PartialEq)]
enum BoxLevel {
	Info,
	Warn,
	Error,
}

fn draw_16_16<D>(l1: &str, l2: &str, top_left: Point, level: BoxLevel, display: &mut D, blink: bool)
where
	D: DrawTarget<Color = BinaryColor>,
	D::Error: Debug,
{

	let (fill_color, text_color) = match level {
		BoxLevel::Info => (BinaryColor::Off, BinaryColor::On),
		BoxLevel::Warn => (BinaryColor::On, BinaryColor::Off),
		BoxLevel::Error => {
			if blink {
				// Blink state: Filled background, inverted text
				(BinaryColor::On, BinaryColor::Off)
			} else {
				// Normal state: Empty background, normal text
				(BinaryColor::Off, BinaryColor::On)
			}
		}
	};

	// Draw rectangle background
	Rectangle::new(top_left, Size::new(16, 16))
		.into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
		.draw(display)
		.unwrap();

	// Draw border
	Rectangle::new(top_left + Point::new(1,1), Size::new(14, 14))
		.into_styled(PrimitiveStyle::with_fill(fill_color))
		.draw(display)
		.unwrap();

	let center_point = top_left + Point::new(7, 2);

	let text_style = TextStyleBuilder::new()
		.alignment(Alignment::Center)
		.baseline(Baseline::Top)
		.build();

	Text::with_text_style(
		&heapless::format!(7; "{}\n{}", &l1[..min(l1.len(), 3)], &l2[..min(l2.len(), 3)]).unwrap(),
		center_point,
		MonoTextStyleBuilder::new().font(&FONT_4X6).text_color(text_color).build(),
		text_style,
	)
		.draw(display)
		.unwrap();
}

const UTC_90DEG: ImageRaw<BinaryColor> = ImageRaw::new(&[
	0x88, // #...#... (C right)
	0x88, // #...#... (C middle)
	0xF8, // #####... (C left/back)
	0x00, // ........ (Spacing)
	0x80, // #....... (T right)
	0xF8, // #####... (T stem)
	0x80, // #....... (T left)
	0x00, // ........ (Spacing)
	0xF8, // #####... (U right)
	0x08, // ....#... (U bottom)
	0xF8, // #####... (U left)
], 6);

const LOC_90DEG: ImageRaw<BinaryColor> = ImageRaw::new(&[
	0x88, // #...#... (C right)
	0x88, // #...#... (C middle)
	0xF8, // #####... (C left/back)
	0x00, // ........ (Spacing)
	0xF8, // #####... (O right)
	0x88, // #...#... (O middle)
	0xF8, // #####... (O left)
	0x00, // ........ (Spacing)
	0x08, // ....#... (L right)
	0x08, // ....#... (L middle)
	0xF8, // #####... (L vertical stem)
], 5);

fn draw_battery_status<D>(
	display: &mut D,
	top_left: Point,
	power: PowerState,
) -> Result<(), D::Error>
where
	D: DrawTarget<Color = BinaryColor>,
{
	Image::new(&BATTERY_ICON, top_left).draw(display)?;

	let mut pct_str: String<4> = String::new();
	match power {
		PowerState::Battery(p) => write!(&mut pct_str, "{}%", p).unwrap(),
		PowerState::Charging(100) => write!(&mut pct_str, "FULL").unwrap(),
		PowerState::Charging(p) => write!(&mut pct_str, "+{}%", p).unwrap(),
		PowerState::VusbOnly => write!(&mut pct_str, "VUSB").unwrap(),
		PowerState::Unknown => write!(&mut pct_str, "---%").unwrap(),
	}

	let text_style = MonoTextStyleBuilder::new()
		.font(&FONT_4X6)
		.text_color(BinaryColor::On)
		.build();

	let text_width = pct_str.len() as i32 * 4; // FONT_4X6 characters are 4px wide
	let x_offset = 1 + (18 - text_width) / 2; // Center inside the 18px battery interior

	// Offset accounts for the border and centering
	Text::with_baseline(
		&pct_str,
		top_left + Point::new(x_offset, 2),
		text_style,
		Baseline::Top,
	)
		.draw(display)?;

	Ok(())
}

// 22 pixels wide, 9 pixels high
const BATTERY_ICON: ImageRaw<BinaryColor> = ImageRaw::new(
	&[
		0xFF, 0xFF, 0xF0, // Y=0:  ####################..  (Top border)
		0x80, 0x00, 0x10, // Y=1:  #..................#..  (Top padding)
		0x80, 0x00, 0x10, // Y=2:  #..................#..  (Text area starts)
		0x80, 0x00, 0x1C, // Y=3:  #..................###  (Nub starts)
		0x80, 0x00, 0x14, // Y=4:  #..................#.#  (Nub hollow center)
		0x80, 0x00, 0x14, // Y=5:  #..................#.#  (Nub hollow center)
		0x80, 0x00, 0x1C, // Y=6:  #..................###  (Nub ends)
		0x80, 0x00, 0x10, // Y=7:  #..................#..  (Text area ends)
		0xFF, 0xFF, 0xF0, // Y=9:  ####################..  (Bottom border)
	],
	22, // width
);