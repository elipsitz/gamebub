use time::{Date, OffsetDateTime, Time};

/// GBA Rtc state
///
/// All in BCD
#[derive(Copy, Clone, Debug)]
pub struct RtcState {
    /// 00 to 99 (representing 2000 to 2099)
    pub year: u8,
    /// 01 to 12
    pub month: u8,
    /// 01 to 31 (adjusted for month length)
    pub day: u8,
    /// 00 to 06
    pub weekday: u8,
    /// 00 to 23 (24-hour) or 00 to 11 (12-hour). Upper bit is AM (0) or PM (1) when 12-hour.
    pub hour: u8,
    /// 00 to 59
    pub minute: u8,
    /// 00 to 59
    pub second: u8,
    /// Status / control
    pub status: u8,
}

impl RtcState {
    pub const STATUS_24H: u8 = 0x40;
    #[allow(unused)]
    pub const STATUS_12H: u8 = 0x0;

    pub fn from_offset_date_time(dt: OffsetDateTime, sunday_offset: u8) -> Self {
        RtcState {
            year: encode_bcd((dt.year() % 100) as u8),
            month: encode_bcd(dt.month() as u8),
            day: encode_bcd(dt.day()),
            weekday: encode_bcd((dt.weekday().number_days_from_sunday() + sunday_offset) % 7),
            hour: encode_bcd(dt.hour()),
            minute: encode_bcd(dt.minute()),
            second: encode_bcd(dt.second()),
            status: Self::STATUS_24H,
        }
    }

    pub fn to_offset_date_time(self) -> Result<(OffsetDateTime, u8), ()> {
        let date = Date::from_calendar_date(
            2000 + decode_bcd(self.year) as i32,
            decode_bcd(self.month).try_into().map_err(|_| ())?,
            decode_bcd(self.day),
        )
        .map_err(|_| ())?;
        let time = Time::from_hms(
            decode_bcd(self.hour),
            decode_bcd(self.minute),
            decode_bcd(self.second),
        )
        .map_err(|_| ())?;
        let dt = OffsetDateTime::new_utc(date, time);
        let stored_weekday = decode_bcd(self.weekday);
        let actual_weekday = dt.weekday().number_days_from_sunday();
        Ok((dt, (stored_weekday + 7 - actual_weekday) % 7))
    }

    /// Convert from FPGA state
    pub fn from_fpga(lo: u32, hi: u32) -> Self {
        // lo=kkddmmyy hi=ccssmmhh
        RtcState {
            year: (lo >> 0) as u8,
            month: (lo >> 8) as u8,
            day: (lo >> 16) as u8,
            weekday: (lo >> 24) as u8,
            hour: (hi >> 0) as u8,
            minute: (hi >> 8) as u8,
            second: (hi >> 16) as u8,
            status: (hi >> 24) as u8,
        }
    }

    pub fn to_fpga(self) -> (u32, u32) {
        let lo = ((self.year as u32) << 0)
            | ((self.month as u32) << 8)
            | ((self.day as u32) << 16)
            | ((self.weekday as u32) << 24);
        let hi = ((self.hour as u32) << 0)
            | ((self.minute as u32) << 8)
            | ((self.second as u32) << 16)
            | ((self.status as u32) << 24);
        (lo, hi)
    }

    pub fn to_disk(self) -> [u8; 8] {
        [
            self.year,
            self.month,
            self.day,
            self.weekday,
            self.hour,
            self.minute,
            self.second,
            self.status,
        ]
    }

    pub fn from_disk(data: [u8; 8]) -> Self {
        RtcState {
            year: data[0],
            month: data[1],
            day: data[2],
            weekday: data[3],
            hour: data[4],
            minute: data[5],
            second: data[6],
            status: data[7],
        }
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
