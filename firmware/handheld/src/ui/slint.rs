use std::cell::Cell;
use std::rc::{Rc, Weak};
use std::time::Instant;

use slint::platform::software_renderer::{
    PremultipliedRgbaColor, RepaintBufferType, SoftwareRenderer, TargetPixel,
};
use slint::platform::{Platform, Renderer, WindowAdapter};
use slint::Window;

static INITIAL_INSTANT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub struct HandheldPlatform {
    pub window: Rc<MinimalSoftwareWindow>,
}

impl Platform for HandheldPlatform {
    fn create_window_adapter(
        &self,
    ) -> Result<std::rc::Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> core::time::Duration {
        let the_beginning = *INITIAL_INSTANT.get_or_init(Instant::now);
        Instant::now() - the_beginning
    }

    fn debug_log(&self, arguments: core::fmt::Arguments) {
        log::info!("{}", arguments);
    }
}

/// Based off of the Slint MinimalSoftwareWindow, with the internal renderer exposed.
pub struct MinimalSoftwareWindow {
    window: Window,
    pub renderer: SoftwareRenderer,
    needs_redraw: Cell<bool>,
    size: Cell<slint::PhysicalSize>,
}

impl MinimalSoftwareWindow {
    pub fn new(repaint_buffer_type: RepaintBufferType) -> Rc<Self> {
        Rc::new_cyclic(|w: &Weak<Self>| Self {
            window: Window::new(w.clone()),
            renderer: SoftwareRenderer::new_with_repaint_buffer_type(repaint_buffer_type),
            needs_redraw: Default::default(),
            size: Default::default(),
        })
    }

    pub fn draw_if_needed(&self, render_callback: impl FnOnce(&SoftwareRenderer)) -> bool {
        if self.needs_redraw.replace(false) {
            render_callback(&self.renderer);
            true
        } else {
            false
        }
    }
}

impl WindowAdapter for MinimalSoftwareWindow {
    fn window(&self) -> &Window {
        &self.window
    }

    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }

    fn size(&self) -> slint::PhysicalSize {
        self.size.get()
    }
    fn set_size(&self, size: slint::WindowSize) {
        self.size.set(size.to_physical(1.));
        self.window
            .dispatch_event(slint::platform::WindowEvent::Resized {
                size: size.to_logical(1.),
            })
    }

    fn request_redraw(&self) {
        self.needs_redraw.set(true);
    }
}

impl core::ops::Deref for MinimalSoftwareWindow {
    type Target = Window;
    fn deref(&self) -> &Self::Target {
        &self.window
    }
}

slint::include_modules!();

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
#[repr(transparent)]
pub struct Argb1555(u16);

impl Argb1555 {
    #![allow(dead_code)]
    pub const TRANSPARENT: Self = Argb1555(0);

    const A_MASK: u16 = 0b1000_0000_0000_0000;
    const R_MASK: u16 = 0b0111_1100_0000_0000;
    const G_MASK: u16 = 0b0000_0011_1110_0000;
    const B_MASK: u16 = 0b0000_0000_0001_1111;

    /// Return the red component in the range 0..=255
    pub fn red(self) -> u8 {
        ((self.0 & Self::R_MASK) >> 7) as u8
    }

    /// Return the green component in the range 0..=255
    pub fn green(self) -> u8 {
        ((self.0 & Self::G_MASK) >> 2) as u8
    }

    /// Return the blue component in the range 0..=255
    pub fn blue(self) -> u8 {
        ((self.0 & Self::B_MASK) << 3) as u8
    }

    pub fn as_u16(self) -> u16 {
        self.0
    }
}

impl TargetPixel for Argb1555 {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let a = (((u8::MAX - color.alpha) as u32) + 4) >> 3;

        // NEW: 000000ggggg000000rrrrr00000bbbbb
        let expanded = (self.0 & (Self::R_MASK | Self::B_MASK)) as u32
            | (((self.0 & Self::G_MASK) as u32) << 16);

        // NEW: 0gggggggg000rrrrrrrr00bbbbbbbb00
        let c =
            ((color.red as u32) << 12) | ((color.green as u32) << 23) | ((color.blue as u32) << 2);

        // NEW: 0ggggg000000rrrrr00000bbbbb00000
        let c = c & 0b01111100000011111000001111100000;

        let res = expanded * a + c;
        let res_a = (((color.alpha as u16) << 8) | self.0) & Self::A_MASK;

        self.0 = res_a
            | ((res >> 21) as u16 & Self::G_MASK)
            | ((res >> 5) as u16 & (Self::R_MASK | Self::B_MASK));
    }

    /// Create a pixel from RGB888.
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self(
            Self::A_MASK
                | (((r as u16) << 7) & Self::R_MASK)
                | (((g as u16) << 2) & Self::G_MASK)
                | ((b as u16) >> 3),
        )
    }

    fn background() -> Self {
        Self::TRANSPARENT
    }
}
