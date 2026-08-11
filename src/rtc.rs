use esp_hal::Async;
use esp_hal::i2c::master::I2c;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

#[embassy_executor::task]
pub async fn drive_rtc(i2c: I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>) {

}