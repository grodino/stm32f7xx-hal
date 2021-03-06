//! Inter-Integrated Circuit (I2C) bus
//! For now, only master mode is implemented

// NB : this implementation started as a modified copy of https://github.com/stm32-rs/stm32f1xx-hal/blob/master/src/i2c.rs
#![allow(dead_code)]
#![allow(unused_macros)]
#![allow(unused_imports)]

use micromath::F32Ext;

use crate::time::Hertz;
use crate::gpio::gpiob::{PB10, PB11, PB6, PB7, PB8, PB9};
use crate::gpio::{Alternate, AF4};
use crate::hal::blocking::i2c::{Read, Write, WriteRead};
use nb::Error::{Other, WouldBlock};
use nb::{Error as NbError, Result as NbResult};
use crate::rcc::{Clocks, Enable, Reset, sealed::RccBus, GetBusFreq};
use crate::device::{DWT, I2C1, I2C2};

use cast::{u16, u8};

// FOR DEBUG
static mut ACTIONS: [u8; 255] = [0; 255];
static mut I_ACTION: usize = 0;

/// I2C error
#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    /// Bus error
    Bus,
    /// Arbitration loss
    Arbitration,
    /// No ack received
    Acknowledge,
    /// Overrun/underrun
    Overrun,
    /// Bus is busy
    Busy,
    // Pec, // SMBUS mode only
    // Timeout, // SMBUS mode only
    // Alert, // SMBUS mode only
    #[doc(hidden)]
    _Extensible,
}
/// SPI mode. The user should make sure that the requested frequency can be
/// generated considering the buses clocks.
#[derive(Debug, PartialEq)]
pub enum Mode {
    Standard {
        frequency: Hertz,
    },
    Fast {
        frequency: Hertz,
    },
    FastPlus  {
        frequency: Hertz,
    },
}

impl Mode {
    pub fn standard<F: Into<Hertz>>(frequency: F) -> Self {
        Mode::Standard{frequency: frequency.into()}
    }

    pub fn fast<F: Into<Hertz>>(frequency: F) -> Self {
        Mode::Fast{frequency: frequency.into()}
    }

    pub fn fast_plus<F: Into<Hertz>>(frequency: F) -> Self {
        Mode::FastPlus{frequency: frequency.into()}
    }

    pub fn get_frequency(&self) -> Hertz {
        match self {
            &Mode::Standard { frequency } => frequency,
            &Mode::Fast { frequency } => frequency,
            &Mode::FastPlus { frequency } => frequency,
        }
    }
}

/// Helper trait to ensure that the correct I2C pins are used for the corresponding interface
pub trait Pins<I2C> {
    const REMAP: bool;
}

impl Pins<I2C1> for (PB8<Alternate<AF4>>, PB7<Alternate<AF4>>) {
    const REMAP: bool = false;
}

impl Pins<I2C1> for (PB8<Alternate<AF4>>, PB9<Alternate<AF4>>) {
    const REMAP: bool = true;
}

impl Pins<I2C2> for (PB10<Alternate<AF4>>, PB11<Alternate<AF4>>) {
    const REMAP: bool = false;
}

/// I2C peripheral operating in master mode
pub struct I2c<I2C, PINS> {
    i2c: I2C,
    pins: PINS,
    mode: Mode,
    pclk: u32,
}

/// embedded-hal compatible blocking I2C implementation
pub struct BlockingI2c<I2C, PINS> {
    nb: I2c<I2C, PINS>,
    data_timeout: u32,
}

impl<PINS> I2c<I2C1, PINS> {
    /// Creates a generic I2C1 object on pins PB6 and PB7 or PB8 and PB9 (if remapped)
    pub fn i2c1(
        i2c: I2C1,
        pins: PINS,
        mode: Mode,
        clocks: Clocks,
        apb: &mut <I2C1 as RccBus>::Bus,
    ) -> Self
    where
        PINS: Pins<I2C1>,
    {
        I2c::_i2c1(i2c, pins, mode, clocks, apb)
    }
}

impl<PINS> BlockingI2c<I2C1, PINS> {
    /// Creates a blocking I2C1 object on pins PB6 and PB7 or PB8 and PB9 using the embedded-hal `BlockingI2c` trait.
    pub fn i2c1(
        i2c: I2C1,
        pins: PINS,
        mode: Mode,
        clocks: Clocks,
        apb: &mut <I2C1 as RccBus>::Bus,
        data_timeout_us: u32,
    ) -> Self
    where
        PINS: Pins<I2C1>,
    {
        BlockingI2c::_i2c1(
            i2c,
            pins,
            mode,
            clocks,
            apb,
            data_timeout_us,
        )
    }
}

impl<PINS> I2c<I2C2, PINS> {
    /// Creates a generic I2C2 object on pins PB10 and PB11 using the embedded-hal `BlockingI2c` trait.
    pub fn i2c2(
        i2c: I2C2,
        pins: PINS,
        mode: Mode,
        clocks: Clocks,
        apb: &mut <I2C2 as RccBus>::Bus
    ) -> Self
    where
        PINS: Pins<I2C2>,
    {
        I2c::_i2c2(i2c, pins, mode, clocks, apb)
    }
}

impl<PINS> BlockingI2c<I2C2, PINS> {
    /// Creates a blocking I2C2 object on pins PB10 and PB1
    pub fn i2c2(
        i2c: I2C2,
        pins: PINS,
        mode: Mode,
        clocks: Clocks,
        apb: &mut <I2C2 as RccBus>::Bus,
        data_timeout_us: u32,
    ) -> Self
    where
        PINS: Pins<I2C2>,
    {
        BlockingI2c::_i2c2(
            i2c,
            pins,
            mode,
            clocks,
            apb,
            data_timeout_us,
        )
    }
}

/// Generates a blocking I2C instance from a universal I2C object
fn blocking_i2c<I2C, PINS>(
    i2c: I2c<I2C, PINS>,
    clocks: Clocks,
    data_timeout_us: u32,
) -> BlockingI2c<I2C, PINS> {
    let sysclk_mhz = clocks.sysclk().0 / 1_000_000;
    return BlockingI2c {
        nb: i2c,
        data_timeout: data_timeout_us * sysclk_mhz,
    };
}


macro_rules! check_status_flag {
    ($i2c:expr, $flag:ident, $status:ident) => {{
        let isr = $i2c.isr.read();

        if isr.berr().bit_is_set() {
            $i2c.icr.write(|w| w.berrcf().set_bit());
            Err(Other(Error::Bus))
        } else if isr.arlo().bit_is_set() {
            $i2c.icr.write(|w| w.arlocf().set_bit());
            Err(Other(Error::Arbitration))
        } else if isr.nackf().bit_is_set() {
            $i2c.icr.write(|w| w.stopcf().set_bit().nackcf().set_bit());
            Err(Other(Error::Acknowledge))
        } else if isr.ovr().bit_is_set() {
            $i2c.icr.write(|w| w.stopcf().set_bit().ovrcf().set_bit());
            Err(Other(Error::Overrun))
        } else if isr.$flag().$status() {
            Ok(())
        } else  {
            Err(WouldBlock)
        }
    }};
}

macro_rules! busy_wait {
    ($nb_expr:expr, $exit_cond:expr) => {{
        loop {
            let res = $nb_expr;
            if res != Err(WouldBlock) {
                break res;
            }
            if $exit_cond {
                break res;
            }
        }
    }};
}

macro_rules! busy_wait_cycles {
    ($nb_expr:expr, $cycles:expr) => {{
        let started = DWT::get_cycle_count();
        let cycles = $cycles;
        busy_wait!(
            $nb_expr,
            DWT::get_cycle_count().wrapping_sub(started) >= cycles
        )
    }};
}

// Generate the same code for both I2Cs
macro_rules! hal {
    ($($I2CX:ident: ($i2cX:ident),)+) => {
        $(
            impl<PINS> I2c<$I2CX, PINS> {
                /// Configures the I2C peripheral to work in master mode
                fn $i2cX(
                    i2c: $I2CX,
                    pins: PINS,
                    mode: Mode,
                    clocks: Clocks,
                    apb: &mut <I2C1 as RccBus>::Bus
                ) -> Self {
                    $I2CX::enable(apb);
                    $I2CX::reset(apb);

                    let pclk = <$I2CX as RccBus>::Bus::get_frequency(&clocks).0;

                    assert!(mode.get_frequency().0 <= 400_000);

                    let mut i2c = I2c { i2c, pins, mode, pclk };
                    i2c.init();
                    i2c
                }

                /// Initializes I2C as master. Configures I2C_PRESC, I2C_SCLDEL,
                /// I2C_SDAEL, I2C_SCLH, I2C_SCLL
                ///
                /// For now, only standart mode is implemented
                fn init(&mut self) {
                    // NOTE : operations are in float for better precision,
                    // STM32F7 usually have FPU and this runs only at
                    // initialisation so the footprint of such heavy calculation
                    // occurs only once
                    
                    // Disable I2C during configuration
                    self.i2c.cr1.write(|w| w.pe().disabled());
                    let target_freq_mhz: f32 = self.mode.get_frequency().0 as f32 / 1_000_000.0;

                    // by default, APB clock is selected by RCC for I2C
                    // Set the base clock as pclk1 (all I2C are on APB1)
                    let base_clk_mhz: f32 = self.pclk as f32 / 1_000_000.0;

                    match self.mode {
                        Mode::Standard { .. } => {
                            // In standart mode, t_{SCL High} = t_{SCL Low}
                            // Delays
                            // let sdadel = 2;
                            // let scldel = 4;

                            let sdadel = 2;
                            let scldel = 4;

                            // SCL Low time
                            let scll = (base_clk_mhz / (2.0 * (target_freq_mhz))).ceil();
                            let scll: u8 = match scll  {
                                0.0..=256.0 => scll as u8 - 1,
                                _ => 255,
                            };
                            let fscll_mhz: f32 = base_clk_mhz / (scll as f32 + 1.0);
                            
                            let sclh: u8 = scll;
                            let fsclh_mhz: f32 = base_clk_mhz / (sclh as f32 + 1.0);

                            // Prescaler
                            let presc = base_clk_mhz / fscll_mhz;
                            let presc: u8 = match presc  {
                                0.0..=16.0 => sclh as u8 - 1,
                                _ => 15,
                            };
                            let fpresc_mhz = base_clk_mhz / (presc as f32 + 1.0);

                            // Update with the real values
                            let fscll_mhz: f32 = fpresc_mhz / (scll as f32 + 1.0);
                            let fsclh_mhz: f32 = fpresc_mhz / (sclh as f32 + 1.0);

                            self.i2c.timingr.write(|w| 
                                w.presc()
                                    .bits(presc)
                                    .scll()
                                    .bits(scll)
                                    .sclh()
                                    .bits(sclh)
                                    .sdadel()
                                    .bits(sdadel)
                                    .scldel()
                                    .bits(scldel)
                            );
                        },
                        _ => unimplemented!(),
                    }

                    self.i2c.cr1.modify(|_, w| w.pe().enabled());
                }

                /// Perform an I2C software reset
                fn reset(&mut self) {
                    self.i2c.cr1.write(|w| w.pe().disabled());
                    // wait for disabled
                    while self.i2c.cr1.read().pe().is_enabled() {}

                    // Re-enable
                    self.i2c.cr1.write(|w| w.pe().enabled());
                }

                /// Set (7-bit) slave address, bus direction (write or read),
                /// generate START condition and set address.
                ///
                /// The user has to specify the number `n_bytes` of bytes to
                /// read. The peripheral automatically waits for the bus to be
                /// free before sending the START and address
                ///
                /// Data transferts of more than 255 bytes are not yet
                /// supported, 10-bit slave address are not yet supported
                fn start(&self, addr: u8, n_bytes: u8, read: bool, auto_stop: bool) { 
                    self.i2c.cr2.write(|mut w| {
                        // Setup data
                        w = w.sadd()
                            .bits(u16(addr << 1 | 0))
                            .add10().clear_bit()
                            .nbytes()
                            .bits(n_bytes as u8)
                            .start()
                            .set_bit();
                        
                        // Setup transfert direction
                        w = match read {
                            true => w.rd_wrn().read(),
                            false => w.rd_wrn().write()
                        };

                        // setup auto-stop
                        match auto_stop {
                            true => w.autoend().automatic(),
                            false => w.autoend().software(),
                        }
                    });
                }

                /// Releases the I2C peripheral and associated pins
                pub fn free(self) -> ($I2CX, PINS) {
                    (self.i2c, self.pins)
                }
            }

            impl<PINS> BlockingI2c<$I2CX, PINS> {
                fn $i2cX(
                    i2c: $I2CX,
                    pins: PINS,
                    mode: Mode,
                    clocks: Clocks,
                    apb: &mut <$I2CX as RccBus>::Bus,
                    data_timeout_us: u32
                ) -> Self {
                    blocking_i2c(I2c::$i2cX(i2c, pins, mode, clocks, apb),
                        clocks, data_timeout_us)
                }

                /// Wait for a byte to be read and return it (ie for RXNE flag
                /// to be set) 
                fn wait_byte_read(&self) -> NbResult<u8, Error> {
                    // Wait until we have received something
                    busy_wait_cycles!(
                        check_status_flag!(self.nb.i2c, rxne, is_not_empty),
                        self.data_timeout
                    )?;

                    Ok(self.nb.i2c.rxdr.read().rxdata().bits())
                }

                /// Wait the write data register to be empty  (ie for TXIS flag
                /// to be set) and write the byte to it
                fn wait_byte_write(&self, byte: u8) -> NbResult<(), Error> {
                    // Wait until we are allowed to send data
                    // (START has been ACKed or last byte when through)
                    busy_wait_cycles!(
                        check_status_flag!(self.nb.i2c, txis, is_empty),
                        self.data_timeout
                    )?;

                    // Put byte on the wire
                    self.nb.i2c.txdr.write(|w| w.txdata().bits(byte));

                    Ok(())
                }
                
                /// Wait for any previous address sequence to end automatically.
                fn wait_start(&self) {
                    while self.nb.i2c.cr2.read().start().bit_is_set() {};
                }
            }

            impl<PINS> Write for BlockingI2c<$I2CX, PINS> {
                type Error = NbError<Error>;

                /// Write bytes to I2C. Currently, `bytes.len()` must be less or
                /// equal than 255
                fn write(&mut self, addr: u8, bytes: &[u8]) -> Result<(), Self::Error> {
                    // TODO support transfers of more than 255 bytes
                    assert!(bytes.len() < 256 && bytes.len() > 0);

                    // Wait for any previous address sequence to end
                    // automatically. This could be up to 50% of a bus
                    // cycle (ie. up to 0.5/freq)
                    self.wait_start();

                    // Set START and prepare to send `bytes`. The
                    // START bit can be set even if the bus is BUSY or
                    // I2C is in slave mode.
                    self.nb.start(addr, bytes.len() as u8, false, true);

                    for byte in bytes {
                        self.wait_byte_write(*byte)?;                       
                    }
                    // automatic STOP

                    Ok(())
                }
            }

            impl<PINS> Read for BlockingI2c<$I2CX, PINS> {
                type Error = NbError<Error>;

                /// Reads enough bytes from slave with `address` to fill `buffer`
                fn read(&mut self, addr: u8, buffer: &mut [u8]) -> Result<(), Self::Error> {
                    // TODO support transfers of more than 255 bytes
                    assert!(buffer.len() < 256 && buffer.len() > 0);

                    // Wait for any previous address sequence to end
                    // automatically. This could be up to 50% of a bus
                    // cycle (ie. up to 0.5/freq)
                    self.wait_start();

                    // Set START and prepare to receive bytes into
                    // `buffer`. The START bit can be set even if the bus
                    // is BUSY or I2C is in slave mode.
                    self.nb.start(addr, buffer.len() as u8, true, true);

                    for byte in buffer {
                        *byte = self.wait_byte_read()?;
                    }

                    // automatic STOP

                    Ok(())
                }
            }

            impl<PINS> WriteRead for BlockingI2c<$I2CX, PINS> {
                type Error = NbError<Error>;

                fn write_read(
                    &mut self,
                    addr: u8,
                    bytes: &[u8],
                    buffer: &mut [u8],
                ) -> Result<(), Self::Error> {
                    // TODO support transfers of more than 255 bytes
                    assert!(bytes.len() < 256 && bytes.len() > 0);
                    assert!(buffer.len() < 256 && buffer.len() > 0);

                    // Start and make sure we don't send STOP after the write
                    self.wait_start();
                    self.nb.start(addr, bytes.len() as u8, false, false);

                    for byte in bytes {
                        self.wait_byte_write(*byte)?;                       
                    }

                    // Wait until the write finishes before beginning to read.
                    // busy_wait2!(self.nb.i2c, tc, is_complete);
                    busy_wait_cycles!(
                        check_status_flag!(self.nb.i2c, tc, is_complete),
                        self.data_timeout
                    )?;

                    // reSTART and prepare to receive bytes into `buffer`
                    self.nb.start(addr, buffer.len() as u8, true, true);
                    
                    for byte in buffer {
                        *byte = self.wait_byte_read()?;
                    }
                    // automatic STOP

                    Ok(())
                }
            }
        )+
    }
}

hal! {
    I2C1: (_i2c1),
    I2C2: (_i2c2),
}
