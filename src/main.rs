#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]
#![deny(clippy::disallowed_types, reason = "Use allocator_api2")]

use core::hint::black_box;
use bt_hci::controller::ExternalController;
use defmt::{info};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::ble::controller::BleConnector;
use panic_rtt_target as _;
use trouble_host::prelude::*;

extern crate alloc;

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 1;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32s3 -o stack-smashing-protection -o unstable-hal -o embassy -o esp32s3-wroom-1-octal-psram -o probe-rs -o defmt -o panic-rtt-target -o embedded-test -o esp -o alloc -o ble-trouble

    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // The following pins are used to bootstrap the chip. They are available
    // for use, but check the datasheet of the module for more information on them.
    // - GPIO0
    // - GPIO3
    // - GPIO45
    // - GPIO46
    // These GPIO pins are in use by some feature of the module and should not be used.
    let _ = peripherals.GPIO27;
    let _ = peripherals.GPIO28;
    let _ = peripherals.GPIO29;
    let _ = peripherals.GPIO30;
    let _ = peripherals.GPIO31;
    let _ = peripherals.GPIO32;
    let _ = peripherals.GPIO33;
    let _ = peripherals.GPIO34;
    let _ = peripherals.GPIO35;
    let _ = peripherals.GPIO36;
    let _ = peripherals.GPIO37;

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 73744);
    // Initialize the PSRAM and also add it to the heap
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);
    let mut testvec: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(73744 * 2);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");
    testvec.push(black_box(0));
    info!("test {}", testvec.as_ptr());

    // find more examples https://github.com/embassy-rs/trouble/tree/main/examples/esp32
    let transport = BleConnector::new(peripherals.BT, Default::default()).unwrap();
    let ble_controller = ExternalController::<_, 1>::new(transport);
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();
    let _stack = trouble_host::new(ble_controller, &mut resources);

    // TODO: Spawn some tasks
    let _ = spawner;

    loop {
        info!("Hello world!");
        Timer::after(Duration::from_secs(1)).await;
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}
