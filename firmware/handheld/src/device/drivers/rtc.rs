#![allow(dead_code)]

use thiserror::Error;

use embedded_hal::i2c::I2c;

const ADDRESS: u8 = 0x51;

#[derive(Debug, Error)]
pub enum Error {
    #[error("i2c error")]
    I2cError,
}

pub struct PCF8563<I2C: I2c> {
    i2c: I2C,
}

impl<I2C> PCF8563<I2C>
where
    I2C: I2c,
{
    pub fn new(i2c: I2C) -> Self {
        PCF8563 { i2c }
    }

    /// Returns the current Datetime, or None if the time is unreliable.
    pub fn read_datetime(&mut self) -> Result<Option<Datetime>, Error> {
        let mut data = [0u8; 7];
        self.i2c
            .write_read(ADDRESS, &[0x02], &mut data)
            .map_err(|_| Error::I2cError)?;

        let datetime = Datetime {
            seconds: decode_bcd(data[0] & 0x7F),
            minutes: decode_bcd(data[1] & 0x7F),
            hours: decode_bcd(data[2] & 0x3F),
            days: decode_bcd(data[3] & 0x3F),
            weekdays: decode_bcd(data[4] & 0x07),
            months: decode_bcd(data[5] & 0x1F),
            years: (decode_bcd(data[6]) as u16) + (100 * (data[5] >> 7) as u16),
        };

        // Check the VL seconds flag and return None if it's invalid.
        let unreliable = (data[0] & 0x80) != 0;

        if unreliable || !datetime.is_valid() {
            Ok(None)
        } else {
            Ok(Some(datetime))
        }
    }

    /// Write the Datetime to the RTC.
    pub fn write_datetime(&mut self, datetime: Datetime) -> Result<(), Error> {
        let data = [
            0x2, // Register to write
            encode_bcd(datetime.seconds),
            encode_bcd(datetime.minutes),
            encode_bcd(datetime.hours),
            encode_bcd(datetime.days),
            encode_bcd(datetime.weekdays),
            encode_bcd(datetime.months) | (if datetime.years >= 100 { 0x80 } else { 0x00 }),
            encode_bcd((datetime.years % 100) as u8),
        ];
        self.i2c.write(ADDRESS, &data).map_err(|_| Error::I2cError)
    }
}

fn decode_bcd(bcd: u8) -> u8 {
    let ones = bcd & 0xF;
    let tens = (bcd & 0xF0) >> 4;
    (10 * tens) + ones
}

fn encode_bcd(data: u8) -> u8 {
    let ones = data % 10;
    let tens: u8 = data / 10;
    (tens << 4) | ones
}

#[derive(Copy, Clone, Debug)]
pub struct Datetime {
    pub seconds: u8,
    pub minutes: u8,
    pub hours: u8,
    pub days: u8,
    pub weekdays: u8,
    pub months: u8,
    pub years: u16,
}
impl Default for Datetime {
    fn default() -> Self {
        Self {
            seconds: 0,
            minutes: 0,
            hours: 0,
            days: 1,
            weekdays: 0,
            months: 1,
            years: 0,
        }
    }
}

/// Unix timestamp of Jan 1, 2000
pub const TIMESTAMP_2000: u64 = 946684800;

impl Datetime {
    /// Returns whether this is a valid datetime for the PCF8563.
    pub fn is_valid(self) -> bool {
        (self.seconds < 60)
            && (self.minutes < 60)
            && (self.hours < 24)
            && (self.days >= 1)
            && (self.days <= 31)
            && (self.weekdays < 7)
            && (self.months >= 1)
            && (self.months <= 12)
            && (self.years < 200)
    }

    /// Convert the Datetime to a Unix timestamp, assuming it starts at year 2000.
    pub fn as_timestamp(self) -> Option<u64> {
        if !self.is_valid() {
            return None;
        }

        let mut timestamp = TIMESTAMP_2000;
        timestamp += self.seconds as u64;
        timestamp += (self.minutes as u64) * 60;
        timestamp += (self.hours as u64) * 3600;

        let month_lookup = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let mut days = (self.days as u64) - 1;
        days += month_lookup[(self.months - 1) as usize];
        if self.months > 2 && self.years.is_multiple_of(4) {
            days += 1;
        }
        days += ((self.years as u64) * 365) + (self.years as u64).div_ceil(4);
        timestamp += days * 86400;

        Some(timestamp)
    }

    pub fn from_timestamp(ts: u64) -> Option<Self> {
        if ts < TIMESTAMP_2000 {
            return None;
        }
        let ts = ts - TIMESTAMP_2000;

        let second = (ts % 86400) as u32;
        let mut day = (ts / 86400) as u32;

        let mut dt = Self::default();
        dt.seconds = (second % 60) as u8;
        dt.minutes = ((second / 60) % 60) as u8;
        dt.hours = (second / 3600) as u8;
        dt.years = ((day * 4) / 1461) as u16;
        day -= ((dt.years as u32) * 1461).div_ceil(4);

        if (dt.years % 4) == 0 {
            day += if day > 59 { 1 } else { 0 }
        } else {
            day += if day > 58 { 2 } else { 0 }
        }
        dt.months = ((((day * 12) + 6) / 367) + 1) as u8;
        dt.days = (day + 1 - ((((dt.months as u32) - 1) * 367) + 5) / 12) as u8;
        Some(dt)
    }
}
