use embassy_time::{Duration, Timer};
use embedded_hal_async::spi::SpiDevice;
use oled_async::displayrotation::DisplayRotation;
use crate::status_display::{DISPLAY_ROTATION_SIGNAL, IS_DISPLAY_AWAKE}; 

pub struct MinimalQmi8658<SPI> {
    spi: SPI,
}

impl<SPI: SpiDevice> MinimalQmi8658<SPI> {
    pub async fn init(&mut self) -> Result<(), SPI::Error> {
        let mut whoami = [0u8; 1];
        self.spi.transaction(&mut [
            embedded_hal_async::spi::Operation::Write(&[0x00 | 0x80]),
            embedded_hal_async::spi::Operation::Read(&mut whoami),
        ]).await?;
        defmt::info!("QMI8658 WHO_AM_I: 0x{:02X}", whoami[0]);

        // CTRL1 (0x02): Enable Address Auto-Increment (Bit 6) and Block Data Update (Bit 5)
        self.write_reg(0x02, 0x60).await?;
        
        // CTRL2 (0x03): Accel Config: 2g full scale (0b000 << 4) | 125 Hz ODR (0x03)
        self.write_reg(0x03, 0x03).await?; 

        // CTRL5 (0x06): Enable Accel Low-Pass Filter (Bit 0) with Mode 0 (2.66% of ODR)
        self.write_reg(0x06, 0x01).await?;

        // CTRL7 (0x08): Enable Accel
        self.write_reg(0x08, 0x01).await?; 
        Ok(())
    }

    pub async fn sleep(&mut self) -> Result<(), SPI::Error> {
        self.write_reg(0x08, 0x00).await?; // Disable Accel to save power
        Ok(())
    }

    pub async fn read_accel(&mut self) -> Result<[i16; 3], SPI::Error> {
        let mut buf = [0u8; 6];
        self.spi.transaction(&mut [
            embedded_hal_async::spi::Operation::Write(&[0x35 | 0x80]),
            embedded_hal_async::spi::Operation::Read(&mut buf),
        ]).await?;
        let x = i16::from_le_bytes([buf[0], buf[1]]);
        let y = i16::from_le_bytes([buf[2], buf[3]]);
        let z = i16::from_le_bytes([buf[4], buf[5]]);
        Ok([x, y, z])
    }
    
    async fn write_reg(&mut self, reg: u8, val: u8) -> Result<(), SPI::Error> {
        self.spi.write(&[reg & 0x7F, val]).await
    }
}

use esp_hal::spi::master::Spi;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use esp_hal::Async;
use esp_hal::gpio::Output;

#[embassy_executor::task]
pub async fn run_imu(spi_device: embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice<'static, CriticalSectionRawMutex, Spi<'static, Async>, Output<'static>>) {
    let mut imu = MinimalQmi8658 { spi: spi_device };
    
    let mut current_rotation = 0;
    let mut candidate_rotation = 0;
    let mut consecutive_count = 0;
    let mut imu_is_asleep = true;

    loop {
        if IS_DISPLAY_AWAKE.load(core::sync::atomic::Ordering::Relaxed) {
            if imu_is_asleep {
                let _ = imu.init().await; 
                imu_is_asleep = false;
            }

            if let Ok(accel) = imu.read_accel().await {
                // Determine which axis is experiencing the strongest pull of gravity
                let new_rot = if accel[2].abs() > accel[0].abs() && accel[2].abs() > accel[1].abs() {
                    // Device is mostly flat, keep the current candidate to prevent noise triggering flips
                    candidate_rotation
                } else if accel[0].abs() > accel[1].abs() {
                    if accel[0] > 0 { 2 } else { 0 }
                } else {
                    if accel[1] > 0 { 1 } else { 3 }
                };

                if new_rot == candidate_rotation {
                    consecutive_count += 1;
                    // Require 3 consecutive readings (1.5 seconds)
                    if consecutive_count >= 3 && current_rotation != candidate_rotation {
                        current_rotation = candidate_rotation;
                        let rot_enum = match current_rotation {
                            1 => DisplayRotation::Rotate90,
                            2 => DisplayRotation::Rotate180,
                            3 => DisplayRotation::Rotate270,
                            _ => DisplayRotation::Rotate0,
                        };
                        DISPLAY_ROTATION_SIGNAL.signal(rot_enum);
                    }
                } else {
                    candidate_rotation = new_rot;
                    consecutive_count = 1;
                }
            }
        } else {
            if !imu_is_asleep {
                let _ = imu.sleep().await; 
                imu_is_asleep = true;
            }
        }
        
        Timer::after(Duration::from_millis(500)).await;
    }
}
