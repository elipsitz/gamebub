use std::time::Duration;

use esp_idf_svc::hal::units::MegaHertz;

use crate::device::{drivers::fpga, Device};

/// Color correction parameters
#[derive(Clone)]
pub struct ColorCorrection {
    in_gamma: f32,
    out_gamma: f32,
    luminance: f32,
    /// [Red] [Green] [Blue]
    matrix: [f32; 9],
}

impl ColorCorrection {
    /// Adjust input gamma: positive to darken, negative to brighten
    #[allow(unused)]
    pub fn adjust_brightness(&self, delta: f32) -> ColorCorrection {
        let mut correction = self.clone();
        correction.in_gamma += delta;
        correction
    }

    /// Apply the user's game color temperature setting.
    pub fn with_color_temperature(&self) -> ColorCorrection {
        // Red, green, and blue gains (linear).
        let gains: [f32; 3] = match crate::kvs::keys::GAME_COLOR_TEMPERATURE.get().unwrap_or(0) {
            1 => [1.0, 0.95, 0.85],
            2 => [1.0, 0.90, 0.74],
            3 => [0.90, 0.95, 1.0],
            _ => return self.clone(),
        };
        let mut correction = self.clone();
        for (row, gain) in correction.matrix.chunks_mut(3).zip(gains) {
            for x in row {
                *x *= gain;
            }
        }
        correction
    }
}

#[allow(unused)]
pub mod presets {
    use super::ColorCorrection;

    /// Identity
    pub static IDENTITY: ColorCorrection = ColorCorrection {
        in_gamma: 1.0,
        out_gamma: 1.0,
        luminance: 1.0,
        matrix: [
            1.0, 0.0, 0.0, // red
            0.0, 1.0, 0.0, // green
            0.0, 0.0, 1.0, // blue
        ],
    };

    /// Game Boy Color and Game Boy Advance (original)
    /// Values from Pokefan531 and hunterk
    pub static GBC_GBA: ColorCorrection = ColorCorrection {
        in_gamma: 2.2,
        out_gamma: 2.2,
        luminance: 0.91,
        matrix: [
            0.905, 0.195, -0.1, // red
            0.1, 0.65, 0.25, // green
            0.1575, 0.1425, 0.7, // blue
        ],
    };

    /// GBA SP (AGS-101)
    /// Values from Pokefan531 and hunterk
    pub static GBA_AGS101: ColorCorrection = ColorCorrection {
        in_gamma: 2.2,
        out_gamma: 2.2,
        luminance: 0.935,
        matrix: [
            0.96, 0.11, -0.07, // red
            0.0325, 0.89, 0.0775, // green
            0.001, -0.03, 1.029, // blue
        ],
    };

    /// Nintendo DS (original)
    /// Values from Pokefan531 and hunterk
    pub static NDS: ColorCorrection = ColorCorrection {
        in_gamma: 2.2,
        out_gamma: 2.2,
        luminance: 0.905,
        matrix: [
            0.835, 0.27, -0.105, // red
            0.10, 0.6375, 0.2625, // green
            0.105, 0.175, 0.72, // blue
        ],
    };

    /// Nintendo DS Lite
    /// Values from Pokefan531 and hunterk
    pub static NDS_LITE: ColorCorrection = ColorCorrection {
        in_gamma: 2.2,
        out_gamma: 2.2,
        luminance: 0.935,
        matrix: [
            0.93, 0.14, -0.07, // red
            0.025, 0.90, 0.075, // green
            0.008, -0.03, 1.022, // blue
        ],
    };

    /// Nintendo Switch Online GBA
    /// Values from Pokefan531 and hunterk
    pub static NSO_GBA: ColorCorrection = ColorCorrection {
        in_gamma: 2.2 + 0.8,
        out_gamma: 2.2,
        luminance: 1.0,
        matrix: [
            0.865, 0.1225, 0.0125, // red
            0.0575, 0.925, 0.0125, // green
            0.0575, 0.1225, 0.82, // blue
        ],
    };
}

impl ColorCorrection {
    pub fn configure(&self, device: &mut Device, base: u32) -> Result<(), fpga::Error> {
        fn make_gamma_table(table: &mut [u16], gamma: f32, luminance: f32, depth: usize) {
            for i in 0..table.len() {
                // Normalized 0.0 to 1.0
                let normal = (i as f32) / ((table.len() - 1) as f32);
                let linear = normal.powf(gamma);
                let screen = linear * luminance;
                // Convert back to float: clamp(floor(f * X), 0, X - 1)
                table[i] = ((screen * ((1 << depth) as f32)) as u16).clamp(0, (1 << depth) - 1);
            }
        }

        let matrix_depth = 12;
        let matrix = self.matrix.map(|x| {
            let value = x * ((1 << matrix_depth) as f32);
            value as i16 as u16
        });

        const INPUT_DEPTH: usize = 5;
        let internal_depth = 12;
        let mut input_table = [0u16; 1 << INPUT_DEPTH];
        make_gamma_table(
            &mut input_table,
            self.in_gamma,
            self.luminance,
            internal_depth,
        );

        const OUTPUT_TABLE_DEPTH: usize = 10;
        let output_depth = 8;
        let mut output_table = [0u16; 1 << OUTPUT_TABLE_DEPTH];
        make_gamma_table(&mut output_table, 1.0 / self.out_gamma, 1.0, output_depth);

        let writes: [(u32, &[u16]); 3] = [
            (0x0000, &matrix),
            (0x4000, &input_table),
            (0x8000, &output_table),
        ];
        for (mut register, data) in writes {
            for chunk in data.chunks(64) {
                let data: &[u8] =
                    unsafe { std::slice::from_raw_parts(chunk.as_ptr().cast(), chunk.len() * 2) };
                let command = fpga::SpiCommand::new(fpga::FpgaSpiWordSize::Bits16);
                device.fpga.spi_write(
                    Some(MegaHertz(10).into()),
                    command,
                    base | register,
                    data,
                )?;
                register += data.len() as u32;
                // Writing to these registers takes a while, so wait after each write.
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        Ok(())
    }
}
