//! Mpu6050 sensor driver.
//!
//! Register sheet [here](https://www.invensense.com/wp-content/uploads/2015/02/MPU-6000-Register-Map1.pdf),
//! Data sheet [here](https://www.invensense.com/wp-content/uploads/2015/02/MPU-6500-Datasheet2.pdf)
//! 
//! To use this driver you must provide a concrete `embedded_hal` implementation.
//! This example uses `linux_embedded_hal`
//! ```no_run
//! use mpu6050::*;
// use linux_embedded_hal::{I2cdev, Delay};
// use i2cdev::linux::LinuxI2CError;
//
// fn main() -> Result<(), Mpu6050Error<LinuxI2CError>> {
//     let i2c = I2cdev::new("/dev/i2c-1")
//         .map_err(Mpu6050Error::I2c)?;
//
//     let mut delay = Delay;
//     let mut mpu = Mpu6050::new(i2c);
//
//     mpu.init(&mut delay)?;
//
//     loop {
//         // get roll and pitch estimate
//         let acc = mpu.get_acc_angles()?;
//         println!("r/p: {:?}", acc);
//
//         // get temp
//         let temp = mpu.get_temp()?;
//         println!("temp: {:?}c", temp);
//
//         // get gyro data, scaled with sensitivity
//         let gyro = mpu.get_gyro()?;
//         println!("gyro: {:?}", gyro);
//
//         // get accelerometer data, scaled with sensitivity
//         let acc = mpu.get_acc()?;
//         println!("acc: {:?}", acc);
//     }
// }
//! ```

#![no_std]

pub mod registers;
mod bits;

use crate::registers::Registers::*;
use libm::{powf, atan2f, sqrtf};
use nalgebra::{Vector3, Vector2};
use embedded_hal::{
    blocking::delay::DelayMs,
    blocking::i2c::{Write, WriteRead},
};
use crate::registers::Registers;

/// PI, f32
pub const PI: f32 = core::f32::consts::PI;

/// PI / 180, for conversion to radians
pub const PI_180: f32 = PI / 180.0;

/// Gyro Sensitivity
pub const FS_SEL: (f32, f32, f32, f32) = (131., 65.5, 32.8, 16.4);

/// Accelerometer Sensitivity
pub const AFS_SEL: (f32, f32, f32, f32) = (16384., 8192., 4096., 2048.);

/// Temperature Offset
pub const TEMP_OFFSET: f32 = 36.53;

/// Temperature Sensitivity
pub const TEMP_SENSITIVITY: f32 = 340.;

// Helper struct to convert Sensor measurement range to appropriate values defined in datasheet
struct Sensitivity(f32);

// Converts accelerometer range to correction/scaling factor, see table p. 29 or register sheet
impl From<AccelRange> for Sensitivity {
    fn from(range: AccelRange) -> Sensitivity {
        match range {
            AccelRange::G2 => return Sensitivity(AFS_SEL.0),
            AccelRange::G4 => return Sensitivity(AFS_SEL.1),
            AccelRange::G8 => return Sensitivity(AFS_SEL.2),
            AccelRange::G16 => return Sensitivity(AFS_SEL.3),
        }
    }
}

// Converts gyro range to correction/scaling factor, see table p. 31 or register sheet
impl From<GyroRange> for Sensitivity {
    fn from(range: GyroRange) -> Sensitivity {
        match range {
            GyroRange::DEG250 => return Sensitivity(FS_SEL.0),
            GyroRange::DEG500 => return Sensitivity(FS_SEL.1),
            GyroRange::DEG1000 => return Sensitivity(FS_SEL.2),
            GyroRange::DEG2000 => return Sensitivity(FS_SEL.3),
        }
    }
}

/// Defines accelerometer range/sensivity
pub enum AccelRange {
    G2,
    G4,
    G8,
    G16,
}

/// Defines gyro range/sensitivity
pub enum GyroRange {
    DEG250,
    DEG500,
    DEG1000,
    DEG2000,
}

/// All possible errors in this crate
#[derive(Debug)]
pub enum Mpu6050Error<E> {
    /// I2C bus error
    I2c(E),

    /// Invalid chip ID was read
    InvalidChipId(u8),
}

/// Handles all operations on/with Mpu6050
pub struct Mpu6050<I> {
    i2c: I,
    acc_sensitivity: f32,
    gyro_sensitivity: f32,
}

impl<I, E> Mpu6050<I>
where
    I: Write<Error = E> + WriteRead<Error = E>, 
{
    /// Side effect free constructor with default sensitivies, no calibration
    pub fn new(i2c: I) -> Self {
        Mpu6050 {
            i2c,
            acc_sensitivity: AFS_SEL.0,
            gyro_sensitivity: FS_SEL.0, 
        }
    }

    /// custom sensitivity
    pub fn new_with_sens(i2c: I, arange: AccelRange, grange: GyroRange) -> Self {
        Mpu6050 {
            i2c,
            acc_sensitivity: Sensitivity::from(arange).0,
            gyro_sensitivity: Sensitivity::from(grange).0,
        }
    }

    /// Wakes MPU6050 with all sensors enabled (default)
    fn wake<D: DelayMs<u8>>(&mut self, delay: &mut D) -> Result<(), Mpu6050Error<E>> {
        self.write_byte(POWER_MGMT_1.addr(), 0)?;
        delay.delay_ms(100u8);
        Ok(())
    }

    /// Init wakes MPU6050 and verifies register addr, e.g. in i2c
    pub fn init<D: DelayMs<u8>>(&mut self, delay: &mut D) -> Result<(), Mpu6050Error<E>> {
        self.wake(delay)?;
        self.verify()?;
        Ok(())
    }

    /// Verifies device to address 0x68 with WHOAMI.addr() Register
    fn verify(&mut self) -> Result<(), Mpu6050Error<E>> {
        let address = self.read_byte(WHOAMI.addr())?;
        if address != SLAVE_ADDR.addr() {
            return Err(Mpu6050Error::InvalidChipId(address));
        }
        Ok(())
    }

    /// Roll and pitch estimation from raw accelerometer readings
    /// NOTE: no yaw! no magnetometer present on MPU6050
    pub fn get_acc_angles(&mut self) -> Result<Vector2<f32>, Mpu6050Error<E>> {
        let acc = self.get_acc()?;

        Ok(Vector2::<f32>::new(
            atan2f(acc.y, sqrtf(powf(acc.x, 2.) + powf(acc.z, 2.))),
            atan2f(-acc.x, sqrtf(powf(acc.y, 2.) + powf(acc.z, 2.)))
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
    fn read_rot(&mut self, reg: u8) -> Result<Vector3<f32>, Mpu6050Error<E>> {
        let mut buf: [u8; 6] = [0; 6];
        self.read_bytes(reg, &mut buf)?;

        Ok(Vector3::<f32>::new(
            self.read_word_2c(&buf[0..2]) as f32,
            self.read_word_2c(&buf[2..4]) as f32,
            self.read_word_2c(&buf[4..6]) as f32
        ))
    }

    /// Accelerometer readings in m/s^2
    pub fn get_acc(&mut self) -> Result<Vector3<f32>, Mpu6050Error<E>> {
        let mut acc = self.read_rot(ACC_REGX_H.addr())?;
        acc /= self.acc_sensitivity;

        Ok(acc)
    }

    /// Gyro readings in rad/s
    pub fn get_gyro(&mut self) -> Result<Vector3<f32>, Mpu6050Error<E>> {
        let mut gyro = self.read_rot(GYRO_REGX_H.addr())?;

        gyro *= PI_180 * self.gyro_sensitivity;

        Ok(gyro)
    }

    /// Temp in degrees celcius
    pub fn get_temp(&mut self) -> Result<f32, Mpu6050Error<E>> {
        let mut buf: [u8; 2] = [0; 2];
        self.read_bytes(TEMP_OUT_H.addr(), &mut buf)?;
        let raw_temp = self.read_word_2c(&buf[0..2]) as f32;

        // According to revision 4.2
        Ok((raw_temp / TEMP_SENSITIVITY) + TEMP_OFFSET)
    }

    /// Writes byte to register
    pub fn write_byte(&mut self, reg: u8, byte: u8) -> Result<(), Mpu6050Error<E>> {
        self.i2c.write(SLAVE_ADDR.addr(), &[reg, byte])
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
        bits::set_bit_n(byte[0], bit_n, enable);
        Ok(self.write_byte(reg, byte[0])?)
    }

    /// Read bit n from register
    fn read_bit(&mut self, reg: u8, bit_n: u8) -> Result<u8, Mpu6050Error<E>> {
        let mut byte: [u8; 1] = [0; 1];
        self.read_bytes(reg, &mut byte)?;
        Ok(bits::get_bit_n(&byte, bit_n))
    }

    /// Reads byte from register
    pub fn read_byte(&mut self, reg: u8) -> Result<u8, Mpu6050Error<E>> {
        let mut byte: [u8; 1] = [0; 1];
        self.i2c.write_read(SLAVE_ADDR.addr(), &[reg], &mut byte)
            .map_err(Mpu6050Error::I2c)?;
        Ok(byte[0])
    }

    /// Reads series of bytes into buf from specified reg
    pub fn read_bytes(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), Mpu6050Error<E>> {
        self.i2c.write_read(SLAVE_ADDR.addr(), &[reg], buf)
            .map_err(Mpu6050Error::I2c)?;
        Ok(())
    }
}

