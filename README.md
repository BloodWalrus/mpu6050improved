# `mpu6050` 

This fork readds a dependency on std, swaps nalgebra for glam, adds a builder for Mpu6050, uses glam::Quat for rotation rather than euler angles

## What Works
* Reading the accelerometer, gyroscope, temperature sensor
    * raw
    * scaled
    * roll/pitch estimation
* Motion Detection
* Setting Accel/Gyro Ranges/Sensitivity
* Setting Accel HPF/LPF

## Basic usage 
To use this driver you must provide a concrete `embedded_hal` implementation. Here's a 
[`linux_embedded_hal`](https://github.com/rust-embedded/linux-embedded-hal) example
```rust
use mpu6050::*;
use linux_embedded_hal::{I2cdev, Delay};
use i2cdev::linux::LinuxI2CError;

fn main() -> Result<(), Mpu6050Error<LinuxI2CError>> {
  let i2c = I2cdev::new("/dev/i2c-1")
          .map_err(Mpu6050Error::I2c)?;

  let mut delay = Delay;
  let mut mpu = Mpu6050Builder::new().i2c(i2c).build().unwrap();

  mpu.init(&mut delay)?;

  loop {
    // get roll and pitch estimate
    let acc = mpu.get_acc_angles()?;
    println!("r/p: {:?}", acc);

    // get temp
    let temp = mpu.get_temp()?;
    println!("temp: {:?}c", temp);

    // get gyro data, scaled with sensitivity 
    let gyro = mpu.get_gyro()?;
    println!("gyro: {:?}", gyro);

    // get accelerometer data, scaled with sensitivity
    let acc = mpu.get_acc()?;
    println!("acc: {:?}", acc);
  }
}
```