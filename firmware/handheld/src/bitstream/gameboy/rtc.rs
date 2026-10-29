#[derive(Copy, Clone, Debug)]
pub struct RtcState {
    seconds: u8,
    minutes: u8,
    hours: u8,
    days: u16,
    days_overflow: bool,
    halt: bool,
}

impl RtcState {
    pub fn from_fpga(data: u32) -> Self {
        // FPGA format: hodddddddddhhhhhmmmmmmssssss
        RtcState {
            seconds: ((data >> 0) & 0b111111) as u8,
            minutes: ((data >> 6) & 0b111111) as u8,
            hours: ((data >> 12) & 0b11111) as u8,
            days: ((data >> 17) & 0b111111111) as u16,
            days_overflow: ((data >> 26) & 1) == 1,
            halt: ((data >> 27) & 1) == 1,
        }
    }

    pub fn to_fpga(&self) -> u32 {
        (((self.seconds as u32) & 0b111111) << 0)
            | (((self.minutes as u32) & 0b111111) << 6)
            | (((self.hours as u32) & 0b11111) << 12)
            | (((self.days as u32) & 0b111111111) << 17)
            | ((self.days_overflow as u32) << 26)
            | ((self.halt as u32) << 27)
    }

    pub fn from_disk(data: &[u8; 20]) -> Self {
        let words = [
            u32::from_le_bytes(data[0..4].try_into().unwrap()),
            u32::from_le_bytes(data[4..8].try_into().unwrap()),
            u32::from_le_bytes(data[8..12].try_into().unwrap()),
            u32::from_le_bytes(data[12..16].try_into().unwrap()),
            u32::from_le_bytes(data[16..20].try_into().unwrap()),
        ];
        RtcState {
            seconds: (words[0] & 0b111111) as u8,
            minutes: (words[1] & 0b111111) as u8,
            hours: (words[2] & 0b11111) as u8,
            days: ((words[3] & 0xFF) | ((words[4] & 1) << 8)) as u16,
            halt: ((words[4] >> 6) & 1) == 1,
            days_overflow: ((words[4] >> 7) & 1) == 1,
        }
    }

    pub fn to_disk(&self) -> [u8; 20] {
        let mut data = [0u8; 20];
        data[0..4].copy_from_slice(&u32::to_le_bytes(self.seconds as u32));
        data[4..8].copy_from_slice(&u32::to_le_bytes(self.minutes as u32));
        data[8..12].copy_from_slice(&u32::to_le_bytes(self.hours as u32));
        data[12..16].copy_from_slice(&u32::to_le_bytes((self.days & 0xFF) as u32));
        let last = ((self.days as u32 & 0x100) >> 8)
            | ((self.halt as u32) << 6)
            | ((self.days_overflow as u32) << 7);
        data[16..20].copy_from_slice(&u32::to_le_bytes(last));
        data
    }

    fn compute_ticks(value: u16, ticks: &mut u64, wrap_point: u16, max_value: u16) -> u64 {
        let mut value = value as u64;
        if value >= (wrap_point as u64) {
            let needed = (max_value as u64) - value;
            if *ticks >= needed {
                *ticks -= needed;
                value = 0;
            } else {
                value += *ticks;
                *ticks = 0;
            }
        }
        value += *ticks;
        *ticks = value / (wrap_point as u64);
        value % (wrap_point as u64)
    }

    pub fn advance(&mut self, seconds: u64) {
        let mut ticks = seconds;
        self.seconds = Self::compute_ticks(self.seconds as u16, &mut ticks, 60, 1 << 6) as u8;
        self.minutes = Self::compute_ticks(self.minutes as u16, &mut ticks, 60, 1 << 6) as u8;
        self.hours = Self::compute_ticks(self.hours as u16, &mut ticks, 24, 1 << 5) as u8;
        self.days = Self::compute_ticks(self.days, &mut ticks, 1 << 9, 1 << 9) as u16;
        if ticks > 0 {
            self.days_overflow = true;
        }
    }
}
