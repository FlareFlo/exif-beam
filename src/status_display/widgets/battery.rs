use embedded_graphics::image::{Image, ImageRaw};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, Point, Drawable};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::mono_font::ascii::FONT_4X6;
use embedded_graphics::text::{Baseline, Text};
use heapless::String;
use core::fmt::Write;
use crate::power_management::PowerState;

pub fn draw_battery_status<D>(
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
