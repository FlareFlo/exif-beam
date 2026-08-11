use alloc::string::String;
use allocator_api2::vec::Vec as AllocVec;
use chrono::{Datelike, Timelike};
use core::cell::RefCell;
use core::fmt::Write as _;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering::Relaxed;
use defmt::info;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Delay, Timer};
use embedded_hal_compat::eh0_2::digital::v2::InputPin;
use embedded_sdmmc::{Mode, SdCard, VolumeIdx, VolumeManager};
use embedded_sdmmc::sdcard::spi::AcquireOpts;
use esp_alloc::ExternalMemory;
use esp_hal::{Async, Blocking};
use esp_hal::gpio::{AnyPin, Input, InputConfig, Output, Pull};
use esp_hal::spi::master::Spi;
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDevice as BlockingSpiDevice;

use crate::gps::state::GPS_STATE;
use crate::sd::{GpsTimeSource, SD_STATE, SdStatus};

struct PsramBuffer(AllocVec<u8, ExternalMemory>);
impl core::fmt::Write for PsramBuffer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

pub type SdSpiDevice = BlockingSpiDevice<'static, CriticalSectionRawMutex, Spi<'static, Async>, Output<'static>>;
pub type VolMgrType = VolumeManager<SdCard<SdSpiDevice, Delay>, GpsTimeSource>;

#[embassy_executor::task]
pub async fn gpx_logger(
    pin: AnyPin<'static>, 
    spi_bus_ref: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<Spi<'static, Async>>>,
    sd_cs: Output<'static>,
    spawner: Spawner
) {
	let pin = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
	let mut button = async_button::Button::new(pin, async_button::ButtonConfig::default());

	static SIGNAL: AtomicBool = AtomicBool::new(false);

	let sd_spi_device = BlockingSpiDevice::new(spi_bus_ref, sd_cs);
	spawner.spawn(run_gps_logger(&SIGNAL, sd_spi_device).unwrap());
	
	loop {
		match button.update().await {
			async_button::ButtonEvent::ShortPress { count: _ } => {
				info!("BOOT Button toggled recording!");
                let current = SIGNAL.load(Relaxed);
				SIGNAL.store(!current, Relaxed);
			}
			async_button::ButtonEvent::LongPress => {
                // Do nothing, PMIC button handles shutdown
			}
		}
	}
}

async fn try_init_sd(volume_mgr: &mut VolMgrType) -> bool {
    let mut sd_error = None;
    if let Ok(_bytes) = volume_mgr.device(|d| {
        match d.num_bytes() {
            Ok(b) => Ok(b),
            Err(e) => {
                sd_error = Some(e);
                Err(e)
            }
        }
    }) {
        *SD_STATE.lock().await = SdStatus::Idle;
        true
    } else {
        if let Some(err) = sd_error {
            defmt::error!("SD mount error: {:?}", defmt::Debug2Format(&err));
        }
        *SD_STATE.lock().await = SdStatus::Missing;
        false
    }
}

async fn flush_to_sd(volume_mgr: &mut VolMgrType, buffer: &mut PsramBuffer, filename: &str) -> bool {
    if let Ok(volume) = volume_mgr.open_volume(VolumeIdx(0)) {
        if let Ok(dir) = volume.open_root_dir() {
            if let Ok(mut file) = dir.open_file_in_dir(filename, Mode::ReadWriteCreateOrAppend) {
                let mut offset = 0;
                while offset < buffer.0.len() {
                    let end = (offset + 512).min(buffer.0.len());
                    let _ = file.write(&buffer.0[offset..end]);
                    offset = end;
                    embassy_futures::yield_now().await;
                }
                let _ = file.close();
            }
            let _ = dir.close();
            let _ = volume.close();
            buffer.0.clear();
            return true;
        }
        let _ = volume.close();
    }
    // Failed to open volume or directory, mark uninit
    volume_mgr.device(|d| d.mark_card_uninit());
    false
}

#[embassy_executor::task(pool_size = 1)]
async fn run_gps_logger(
    run: &'static AtomicBool,
    sd_spi_device: SdSpiDevice
) {
    let mut options = AcquireOpts::default();
    options.acquire_retries = 2; // Don't block the executor for 15 seconds if SD is missing
    let sd_card = SdCard::new_with_options(sd_spi_device, Delay, options);
    let mut volume_mgr = VolumeManager::new(sd_card, GpsTimeSource);
    
    let mut init_success = false;
    let mut buffer = PsramBuffer(AllocVec::new_in(ExternalMemory));
    let mut was_recording = false;
    let mut current_filename = String::new();

    loop {
        if !init_success {
            init_success = try_init_sd(&mut volume_mgr).await;
        }
        
        let is_recording = run.load(Relaxed);
        
        if is_recording && !was_recording {
            // Started recording
            was_recording = true;
            let s = GPS_STATE.lock().await;
            current_filename = alloc::format!("{:02}{:02}{:02}{:02}.GPX", s.date.day(), s.time.hour(), s.time.minute(), s.time.second());
            drop(s);
            
            let _ = write!(&mut buffer, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"exif-beam\">\n  <trk>\n    <trkseg>\n");
            *SD_STATE.lock().await = SdStatus::Recording;
        } else if !is_recording && was_recording {
            // Stopped recording
            was_recording = false;
            *SD_STATE.lock().await = SdStatus::Idle;
            let _ = write!(&mut buffer, "    </trkseg>\n  </trk>\n</gpx>\n");
        }

        if is_recording {
            let s = GPS_STATE.lock().await;
            if s.has_fix {
                let _ = write!(&mut buffer, 
                    "      <trkpt lat=\"{:.6}\" lon=\"{:.6}\">\n        <hdop>{:.1}</hdop>\n        <sat>{}</sat>\n      </trkpt>\n",
                    s.lat, s.lon, s.hdop, s.sats
                );
            }
            drop(s);
        }
            
        // If buffer > 10KB, or we just stopped recording and have a closing tag, flush to SD
        if buffer.0.len() > 10_000 || (!is_recording && !buffer.0.is_empty()) {
            if init_success {
                init_success = flush_to_sd(&mut volume_mgr, &mut buffer, current_filename.as_str()).await;
            } else {
                buffer.0.clear(); // Discard if no SD available to avoid memory leak
            }
        }
        
        if init_success {
            Timer::after_millis(1000).await;
        } else {
            Timer::after_millis(5000).await; // Don't spam retries if SD is missing
        }
    }
}
