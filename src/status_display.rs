use embassy_sync::mutex::Mutex;
use display_interface_i2c::I2CInterface;
use oled_async::displays::sh1106::Sh1106_128_64;
use oled_async::Builder;
use oled_async::prelude::GraphicsMode;
use embedded_hal_compat::{ForwardCompat, Reverse};
use core::cell::RefCell;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use chrono::{FixedOffset, Timelike};
use core::fmt::Debug;
use core::cmp::min;
use defmt::info;
use embassy_futures::select::select;
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex};
use embassy_time::{Duration, Timer};
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
use oled_async::displayrotation::DisplayRotation;
use crate::gps::GPS_STATE;
use crate::power_management::get_battery_level;

pub static DISPLAY_SIGNAL: Signal<CriticalSectionRawMutex, DisplayState> = Signal::new();

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
	loop {
		let gps = GPS_STATE.lock().await;
		state.sats = gps.sats;
		state.lat = gps.lat;
		state.lon = gps.lon;
		state.date = gps.date;
		state.time = gps.time;
		drop(gps);

		state.bat = get_battery_level();
		draw_status_display(&mut display, &state);
		yield_now().await;
		display.flush().await.unwrap();
		select(Timer::after(Duration::from_secs(1)), DISPLAY_SIGNAL.wait()).await;
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
	pub bat: u8,
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
		DateTime::<Utc>::from_naive_utc_and_offset(NaiveDateTime::new(self.date, self.time), Utc).with_timezone(&FixedOffset::east_opt(0).expect("Infallible. UTC."))
	}

	pub fn now_local(&self) -> Option<DateTime<FixedOffset>> {
		self.local_time
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

	let mut local_instead_of_utc = true;

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

	let now = if local_instead_of_utc && let Some(time) = state.now_local() {
		// Fall back to UTC when LOC is not available (yet)
		local_instead_of_utc = false;
		time
	} else {
		state.now_utc()
	};
	// Clock
	Text::with_baseline(&heapless::format!(30; " {:02}:{:02}:{:02}", now.hour(), now.minute(), now.second()).unwrap(), Point::new(0, yoffs(2) + 1), TXT.font(&FONT_6X13_BOLD).build(), Baseline::Top)
		.draw(display)
		.unwrap();

	Image::new(&if local_instead_of_utc {LOC_90DEG } else { UTC_90DEG }, Point::new(0, 21)).draw(display).unwrap();
	Text::new(&heapless::format!(5; "{}%", state.bat).unwrap(), Point::new(0, 40), TXT.font(&FONT_6X13_BOLD).build()).draw(display).unwrap();
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