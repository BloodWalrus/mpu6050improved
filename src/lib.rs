//! # Mpu6050 sensor driver.
//!
//! `embedded_hal` based driver with i2c access to MPU6050
//!
//! ### Misc
//! * [Register sheet](https://www.invensense.com/wp-content/uploads/2015/02/MPU-6000-Register-Map1.pdf),
//! * [Data sheet](https://www.invensense.com/wp-content/uploads/2015/02/MPU-6500-Datasheet2.pdf)
//!
//! To use this driver you must provide a concrete `embedded_hal` implementation.
//! This example uses `linux_embedded_hal`.
//!
//! **More Examples** can be found [here](https://github.com/juliangaal/mpu6050/tree/master/examples).
//! ```no_run
//! use mpu6050::*;
//! use linux_embedded_hal::{I2cdev, Delay};
//! use i2cdev::linux::LinuxI2CError;
//!
//! fn main() -> Result<(), Mpu6050Error<LinuxI2CError>> {
//!     let i2c = I2cdev::new("/dev/i2c-1")
//!         .map_err(Mpu6050Error::I2c)?;
//!
//!     let mut delay = Delay;
//!     let mut mpu = Mpu6050::new(i2c);
//!
//!     mpu.init(&mut delay)?;
//!
//!     loop {
//!         // get roll and pitch estimate
//!         let acc = mpu.get_acc_angles()?;
//!         println!("r/p: {:?}", acc);
//!
//!         // get sensor temp
//!         let temp = mpu.get_temp()?;
//!         printlnasd!("temp: {:?}c", temp);
//!
//!         // get gyro data, scaled with sensitivity
//!         let gyro = mpu.get_gyro()?;
//!         println!("gyro: {:?}", gyro);
//!
//!         // get accelerometer data, scaled with sensitivity
//!         let acc = mpu.get_acc()?;
//!         println!("acc: {:?}", acc);
//!     }
//! }
//! ```

mod bits;
pub mod device;

use std::fmt::Display;

use crate::device::*;
use embedded_hal::{
    blocking::delay::DelayMs,
    blocking::i2c::{Write, WriteRead},
};
use glam::EulerRot;
pub use glam::{Quat, Vec3A};

/// PI, f32
pub const PI: f32 = core::f32::consts::PI;

/// PI / 180, for conversion to radians
pub const PI_180: f32 = PI / 180.0;

/// All possible errors for Mpu6050
#[derive(Debug)]
pub enum Mpu6050Error<E> {
    /// I2C bus error
    I2c(E),

    /// Invalid chip ID was read
    InvalidChipId(u8),
}

#[derive(Debug)]
pub enum Mpu6050BuilderError {
    /// No i2c device was provided to the builder
    NoI2cDeviceProvided,
}

impl Display for Mpu6050BuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Mpu6050BuilderError::NoI2cDeviceProvided => "Mpu6050BuilderError::NoI2cDeviceProvided",
        })
    }
}

pub struct Mpu6050Builder<I> {
    i2c: Option<I>,
    slave_addr: Option<u8>,
    acc_sensitivity: Option<AccelRange>,
    gyro_sensitivity: Option<GyroRange>,
    gyro_offset: Option<Vec3A>,
    acc_offset: Option<Vec3A>,
}

impl<I> Mpu6050Builder<I> {
    pub fn new() -> Self {
        Self {
            i2c: None,
            slave_addr: None,
            acc_sensitivity: None,
            gyro_sensitivity: None,
            gyro_offset: None,
            acc_offset: None,
        }
    }

    pub fn i2c(mut self, i2c: I) -> Self {
        self.i2c = Some(i2c);
        self
    }

    pub fn slave_addr(mut self, slave_addr: u8) -> Self {
        self.slave_addr = Some(slave_addr);
        self
    }

    pub fn acc_sensitivity(mut self, acc_sensitivity: AccelRange) -> Self {
        self.acc_sensitivity = Some(acc_sensitivity);
        self
    }

    pub fn gyro_sensitivity(mut self, gyro_sensitivity: GyroRange) -> Self {
        self.gyro_sensitivity = Some(gyro_sensitivity);
        self
    }

    pub fn gyro_offset(mut self, gyro_offset: Vec3A) -> Self {
        self.gyro_offset = Some(gyro_offset);
        self
    }

    pub fn acc_offset(mut self, acc_offset: Vec3A) -> Self {
        self.acc_offset = Some(acc_offset);
        self
    }

    pub fn build(self) -> Result<Mpu6050<I>, Mpu6050BuilderError> {
        Ok(Mpu6050 {
            i2c: match self.i2c {
                Some(i2c) => i2c,
                None => return Err(Mpu6050BuilderError::NoI2cDeviceProvided),
            },
            slave_addr: self.slave_addr.unwrap_or(DEFAULT_SLAVE_ADDR),
            acc_sensitivity: self
                .acc_sensitivity
                .map(|sens| sens.sensitivity())
                .unwrap_or(ACCEL_SENS.0),
            gyro_sensitivity: self
                .gyro_sensitivity
                .map(|sens| sens.sensitivity())
                .unwrap_or(GYRO_SENS.0),
            gyro_offset: self.gyro_offset.unwrap_or(Vec3A::ZERO),
            acc_offset: self.acc_offset.unwrap_or(Vec3A::ZERO),
        })
    }
}

/// Handles all operations on/with Mpu6050
pub struct Mpu6050<I> {
    i2c: I,
    slave_addr: u8,
    acc_sensitivity: f32,
    gyro_sensitivity: f32,
    gyro_offset: Vec3A,
    acc_offset: Vec3A,
}

impl<I, E> Mpu6050<I>
where
    I: Write<Error = E> + WriteRead<Error = E>,
{
    /// Wakes MPU6050 with all sensors enabled (default)
    fn wake<D: DelayMs<u8>>(&mut self, delay: &mut D) -> Result<(), Mpu6050Error<E>> {
        // MPU6050 has sleep enabled by default -> set bit 0 to wake
        // Set clock source to be PLL with x-axis gyroscope reference, bits 2:0 = 001 (See Register Map )
        self.write_byte(PWR_MGMT_1::ADDR, 0x01)?;
        delay.delay_ms(100u8);
        Ok(())
    }

    /// From Register map:
    /// "An  internal  8MHz  oscillator,  gyroscope based  clock,or  external  sources  can  be
    /// selected  as the MPU-60X0 clock source.
    /// When the internal 8 MHz oscillator or an external source is chosen as the clock source,
    /// the MPU-60X0 can operate in low power modes with the gyroscopes disabled. Upon power up,
    /// the MPU-60X0clock source defaults to the internal oscillator. However, it is highly
    /// recommended  that  the  device beconfigured  to  use  one  of  the  gyroscopes
    /// (or  an  external  clocksource) as the clock reference for improved stability.
    /// The clock source can be selected according to the following table...."
    pub fn set_clock_source(&mut self, source: CLKSEL) -> Result<(), Mpu6050Error<E>> {
        Ok(self.write_bits(
            PWR_MGMT_1::ADDR,
            PWR_MGMT_1::CLKSEL.bit,
            PWR_MGMT_1::CLKSEL.length,
            source as u8,
        )?)
    }

    /// get current clock source
    pub fn get_clock_source(&mut self) -> Result<CLKSEL, Mpu6050Error<E>> {
        let source = self.read_bits(
            PWR_MGMT_1::ADDR,
            PWR_MGMT_1::CLKSEL.bit,
            PWR_MGMT_1::CLKSEL.length,
        )?;
        Ok(CLKSEL::from(source))
    }

    /// Init wakes MPU6050 and verifies register addr, e.g. in i2c
    pub fn init<D: DelayMs<u8>>(&mut self, delay: &mut D) -> Result<(), Mpu6050Error<E>> {
        self.wake(delay)?;
        self.verify()?;
        self.set_accel_range(AccelRange::G2)?;
        self.set_gyro_range(GyroRange::D250)?;
        self.set_accel_hpf(ACCEL_HPF::_RESET)?;
        Ok(())
    }

    /// Verifies device to address 0x68 with WHOAMI.addr() Register
    fn verify(&mut self) -> Result<(), Mpu6050Error<E>> {
        let address = self.read_byte(WHOAMI)?;
        if address != DEFAULT_SLAVE_ADDR {
            return Err(Mpu6050Error::InvalidChipId(address));
        }
        Ok(())
    }

    /// setup motion detection
    /// sources:
    /// * https://github.com/kriswiner/MPU6050/blob/a7e0c8ba61a56c5326b2bcd64bc81ab72ee4616b/MPU6050IMU.ino#L486
    /// * https://arduino.stackexchange.com/a/48430
    pub fn setup_motion_detection(&mut self) -> Result<(), Mpu6050Error<E>> {
        self.write_byte(0x6B, 0x00)?;
        // optional? self.write_byte(0x68, 0x07)?; // Reset all internal signal paths in the MPU-6050 by writing 0x07 to register 0x68;
        self.write_byte(INT_PIN_CFG::ADDR, 0x20)?; //write register 0x37 to select how to use the interrupt pin. For an active high, push-pull signal that stays until register (decimal) 58 is read, write 0x20.
        self.write_byte(ACCEL_CONFIG::ADDR, 0x01)?; //Write register 28 (==0x1C) to set the Digital High Pass Filter, bits 3:0. For example set it to 0x01 for 5Hz. (These 3 bits are grey in the data sheet, but they are used! Leaving them 0 means the filter always outputs 0.)
        self.write_byte(MOT_THR, 10)?; //Write the desired Motion threshold to register 0x1F (For example, write decimal 20).
        self.write_byte(MOT_DUR, 40)?; //Set motion detect duration to 1  ms; LSB is 1 ms @ 1 kHz rate
        self.write_byte(0x69, 0x15)?; //to register 0x69, write the motion detection decrement and a few other settings (for example write 0x15 to set both free-fall and motion decrements to 1 and accelerometer start-up delay to 5ms total by adding 1ms. )
        self.write_byte(INT_ENABLE::ADDR, 0x40)?; //write register 0x38, bit 6 (0x40), to enable motion detection interrupt.
        Ok(())
    }

    /// get whether or not motion has been detected (INT_STATUS, MOT_INT)
    pub fn get_motion_detected(&mut self) -> Result<bool, Mpu6050Error<E>> {
        Ok(self.read_bit(INT_STATUS::ADDR, INT_STATUS::MOT_INT)? != 0)
    }

    /// set accel high pass filter mode
    pub fn set_accel_hpf(&mut self, mode: ACCEL_HPF) -> Result<(), Mpu6050Error<E>> {
        Ok(self.write_bits(
            ACCEL_CONFIG::ADDR,
            ACCEL_CONFIG::ACCEL_HPF.bit,
            ACCEL_CONFIG::ACCEL_HPF.length,
            mode as u8,
        )?)
    }

    /// get accel high pass filter mode
    pub fn get_accel_hpf(&mut self) -> Result<ACCEL_HPF, Mpu6050Error<E>> {
        let mode: u8 = self.read_bits(
            ACCEL_CONFIG::ADDR,
            ACCEL_CONFIG::ACCEL_HPF.bit,
            ACCEL_CONFIG::ACCEL_HPF.length,
        )?;

        Ok(ACCEL_HPF::from(mode))
    }

    /// Set gyro range, and update sensitivity accordingly
    pub fn set_gyro_range(&mut self, range: GyroRange) -> Result<(), Mpu6050Error<E>> {
        self.write_bits(
            GYRO_CONFIG::ADDR,
            GYRO_CONFIG::FS_SEL.bit,
            GYRO_CONFIG::FS_SEL.length,
            range as u8,
        )?;

        self.gyro_sensitivity = range.sensitivity();
        Ok(())
    }

    /// get current gyro range
    pub fn get_gyro_range(&mut self) -> Result<GyroRange, Mpu6050Error<E>> {
        let byte = self.read_bits(
            GYRO_CONFIG::ADDR,
            GYRO_CONFIG::FS_SEL.bit,
            GYRO_CONFIG::FS_SEL.length,
        )?;

        Ok(GyroRange::from(byte))
    }

    /// set accel range, and update sensitivy accordingly
    pub fn set_accel_range(&mut self, range: AccelRange) -> Result<(), Mpu6050Error<E>> {
        self.write_bits(
            ACCEL_CONFIG::ADDR,
            ACCEL_CONFIG::FS_SEL.bit,
            ACCEL_CONFIG::FS_SEL.length,
            range as u8,
        )?;

        self.acc_sensitivity = range.sensitivity();
        Ok(())
    }

    /// get current accel_range
    pub fn get_accel_range(&mut self) -> Result<AccelRange, Mpu6050Error<E>> {
        let byte = self.read_bits(
            ACCEL_CONFIG::ADDR,
            ACCEL_CONFIG::FS_SEL.bit,
            ACCEL_CONFIG::FS_SEL.length,
        )?;

        Ok(AccelRange::from(byte))
    }

    /// reset device
    pub fn reset_device<D: DelayMs<u8>>(&mut self, delay: &mut D) -> Result<(), Mpu6050Error<E>> {
        self.write_bit(PWR_MGMT_1::ADDR, PWR_MGMT_1::DEVICE_RESET, true)?;
        delay.delay_ms(100u8);
        // Note: Reset sets sleep to true! Section register map: resets PWR_MGMT to 0x40
        Ok(())
    }

    /// enable, disable sleep of sensor
    pub fn set_sleep_enabled(&mut self, enable: bool) -> Result<(), Mpu6050Error<E>> {
        Ok(self.write_bit(PWR_MGMT_1::ADDR, PWR_MGMT_1::SLEEP, enable)?)
    }

    /// get sleep status
    pub fn get_sleep_enabled(&mut self) -> Result<bool, Mpu6050Error<E>> {
        Ok(self.read_bit(PWR_MGMT_1::ADDR, PWR_MGMT_1::SLEEP)? != 0)
    }

    /// enable, disable temperature measurement of sensor
    /// TEMP_DIS actually saves "disabled status"
    /// 1 is disabled! -> enable=true : bit=!enable
    pub fn set_temp_enabled(&mut self, enable: bool) -> Result<(), Mpu6050Error<E>> {
        Ok(self.write_bit(PWR_MGMT_1::ADDR, PWR_MGMT_1::TEMP_DIS, !enable)?)
    }

    /// get temperature sensor status
    /// TEMP_DIS actually saves "disabled status"
    /// 1 is disabled! -> 1 == 0 : false, 0 == 0 : true
    pub fn get_temp_enabled(&mut self) -> Result<bool, Mpu6050Error<E>> {
        Ok(self.read_bit(PWR_MGMT_1::ADDR, PWR_MGMT_1::TEMP_DIS)? == 0)
    }

    /// set accel x self test
    pub fn set_accel_x_self_test(&mut self, enable: bool) -> Result<(), Mpu6050Error<E>> {
        Ok(self.write_bit(ACCEL_CONFIG::ADDR, ACCEL_CONFIG::XA_ST, enable)?)
    }

    /// get accel x self test
    pub fn get_accel_x_self_test(&mut self) -> Result<bool, Mpu6050Error<E>> {
        Ok(self.read_bit(ACCEL_CONFIG::ADDR, ACCEL_CONFIG::XA_ST)? != 0)
    }

    /// set accel y self test
    pub fn set_accel_y_self_test(&mut self, enable: bool) -> Result<(), Mpu6050Error<E>> {
        Ok(self.write_bit(ACCEL_CONFIG::ADDR, ACCEL_CONFIG::YA_ST, enable)?)
    }

    /// get accel y self test
    pub fn get_accel_y_self_test(&mut self) -> Result<bool, Mpu6050Error<E>> {
        Ok(self.read_bit(ACCEL_CONFIG::ADDR, ACCEL_CONFIG::YA_ST)? != 0)
    }

    /// set accel z self test
    pub fn set_accel_z_self_test(&mut self, enable: bool) -> Result<(), Mpu6050Error<E>> {
        Ok(self.write_bit(ACCEL_CONFIG::ADDR, ACCEL_CONFIG::ZA_ST, enable)?)
    }

    /// get accel z self test
    pub fn get_accel_z_self_test(&mut self) -> Result<bool, Mpu6050Error<E>> {
        Ok(self.read_bit(ACCEL_CONFIG::ADDR, ACCEL_CONFIG::ZA_ST)? != 0)
    }

    /// Roll and pitch estimation from raw accelerometer readings
    /// NOTE: no yaw! no magnetometer present on MPU6050
    /// https://www.nxp.com/docs/en/application-note/AN3461.pdf equation 28, 29
    pub fn get_acc_angles(&mut self) -> Result<Quat, Mpu6050Error<E>> {
        let acc = self.get_acc()?;

        Ok(Quat::from_euler(
            EulerRot::XYZ,
            acc.y.atan2((acc.x.powf(2.0) + acc.z.powf(2.0)).sqrt()),
            (-acc.x).atan2((acc.y.powf(2.0) + acc.z.powf(2.0)).sqrt()),
            0.0,
        ))
    }

    /// Converts 2 bytes number in 2 compliment
    /// TODO i16?! whats 0x8000?!
    fn read_word_2c(&self, byte: &[u8]) -> i32 {
        let high: i32 = byte[0] as i32;
        let low: i32 = byte[1] as i32;
        let mut word: i32 = (high << 8) + low;

        if word >= 0x8000 {
            word = -((65535 - word) + 1);
        }

        word
    }

    /// Reads rotation (gyro/acc) from specified register
    fn read_rot(&mut self, reg: u8) -> Result<Vec3A, Mpu6050Error<E>> {
        let mut buf: [u8; 6] = [0; 6];
        self.read_bytes(reg, &mut buf)?;

        Ok(Vec3A::new(
            self.read_word_2c(&buf[0..2]) as f32,
            self.read_word_2c(&buf[2..4]) as f32,
            self.read_word_2c(&buf[4..6]) as f32,
        ))
    }

    /// Accelerometer readings in g
    pub fn get_acc(&mut self) -> Result<Vec3A, Mpu6050Error<E>> {
        let mut acc = self.read_rot(ACC_REGX_H)?;
        acc /= self.acc_sensitivity;

        Ok(acc + self.acc_offset)
    }

    /// Gyro readings in rad/s
    pub fn get_gyro(&mut self) -> Result<Vec3A, Mpu6050Error<E>> {
        let mut gyro = self.read_rot(GYRO_REGX_H)?;

        gyro *= PI_180 / self.gyro_sensitivity;

        Ok(gyro + self.gyro_offset)
    }

    /// Sensor Temp in degrees celcius
    pub fn get_temp(&mut self) -> Result<f32, Mpu6050Error<E>> {
        let mut buf: [u8; 2] = [0; 2];
        self.read_bytes(TEMP_OUT_H, &mut buf)?;
        let raw_temp = self.read_word_2c(&buf[0..2]) as f32;

        // According to revision 4.2
        Ok((raw_temp / TEMP_SENSITIVITY) + TEMP_OFFSET)
    }

    /// Writes byte to register
    pub fn write_byte(&mut self, reg: u8, byte: u8) -> Result<(), Mpu6050Error<E>> {
        self.i2c
            .write(self.slave_addr, &[reg, byte])
            .map_err(Mpu6050Error::I2c)?;
        // delay disabled for dev build
        // TODO: check effects with physical unit
        // self.delay.delay_ms(10u8);
        Ok(())
    }

    /// Enables bit n at register address reg
    pub fn write_bit(&mut self, reg: u8, bit_n: u8, enable: bool) -> Result<(), Mpu6050Error<E>> {
        let mut byte: [u8; 1] = [0; 1];
        self.read_bytes(reg, &mut byte)?;
        bits::set_bit(&mut byte[0], bit_n, enable);
        Ok(self.write_byte(reg, byte[0])?)
    }

    /// Write bits data at reg from start_bit to start_bit+length
    pub fn write_bits(
        &mut self,
        reg: u8,
        start_bit: u8,
        length: u8,
        data: u8,
    ) -> Result<(), Mpu6050Error<E>> {
        let mut byte: [u8; 1] = [0; 1];
        self.read_bytes(reg, &mut byte)?;
        bits::set_bits(&mut byte[0], start_bit, length, data);
        Ok(self.write_byte(reg, byte[0])?)
    }

    /// Read bit n from register
    fn read_bit(&mut self, reg: u8, bit_n: u8) -> Result<u8, Mpu6050Error<E>> {
        let mut byte: [u8; 1] = [0; 1];
        self.read_bytes(reg, &mut byte)?;
        Ok(bits::get_bit(byte[0], bit_n))
    }

    /// Read bits at register reg, starting with bit start_bit, until start_bit+length
    pub fn read_bits(&mut self, reg: u8, start_bit: u8, length: u8) -> Result<u8, Mpu6050Error<E>> {
        let mut byte: [u8; 1] = [0; 1];
        self.read_bytes(reg, &mut byte)?;
        Ok(bits::get_bits(byte[0], start_bit, length))
    }

    /// Reads byte from register
    pub fn read_byte(&mut self, reg: u8) -> Result<u8, Mpu6050Error<E>> {
        let mut byte: [u8; 1] = [0; 1];
        self.i2c
            .write_read(self.slave_addr, &[reg], &mut byte)
            .map_err(Mpu6050Error::I2c)?;
        Ok(byte[0])
    }

    /// Reads series of bytes into buf from specified reg
    pub fn read_bytes(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), Mpu6050Error<E>> {
        self.i2c
            .write_read(self.slave_addr, &[reg], buf)
            .map_err(Mpu6050Error::I2c)?;
        Ok(())
    }
}
